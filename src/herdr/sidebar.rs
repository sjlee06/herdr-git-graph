use super::{SIDEBAR_WIDTH, context_id, open_view, plugin_context};
use crate::theme::ThemeMode;
use anyhow::{Context, Result, ensure};
use clap::ValueEnum;
use serde_json::{Value, json};
use std::{
    collections::hash_map::DefaultHasher,
    env,
    ffi::OsStr,
    fs::{self, File, OpenOptions},
    hash::{Hash, Hasher},
    io::Write,
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

// One lock per server/workspace serializes open actions and asynchronous hooks.
// Keep the lock file in place: unlinking it could let another process lock a
// different inode while an earlier invocation still holds the original lock.
struct SidebarState {
    _lock: File,
    path: PathBuf,
}

impl SidebarState {
    fn lock(workspace: &str) -> Result<Self> {
        let directory = env::var_os("HERDR_PLUGIN_STATE_DIR")
            .map(PathBuf::from)
            .or_else(|| {
                env::var_os("XDG_STATE_HOME")
                    .map(PathBuf::from)
                    .or_else(|| env::var_os("HOME").map(|p| PathBuf::from(p).join(".local/state")))
                    .map(|p| p.join("herdr-git-graph"))
            })
            .context("사이드바 상태를 저장할 경로가 없습니다.")?
            .join("shared-sidebars");
        fs::create_dir_all(&directory)?;
        let mut identity = DefaultHasher::new();
        env::var_os("HERDR_SOCKET_PATH").hash(&mut identity);
        workspace.hash(&mut identity);
        let base = directory.join(format!("{:016x}", identity.finish()));
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(base.with_extension("lock"))?;
        lock.lock()?;
        Ok(Self {
            _lock: lock,
            path: base.with_extension("json"),
        })
    }

    fn read(&self) -> Result<Option<Value>> {
        match fs::read(&self.path) {
            Ok(bytes) => Ok(serde_json::from_slice(&bytes).ok()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    fn save(&self, pane: &Value, record: &Value) -> Result<()> {
        // Keep launch settings across moves, and remember the containing tab
        // so tab deletion can be distinguished from closing only the sidebar.
        // Terminal identity protects against pane IDs reused after a restart.
        let mut record = record.clone();
        for key in ["pane_id", "terminal_id", "tab_id"] {
            record[key] = json!(string(pane, key)?);
        }
        self.write(&record)
    }

    fn write(&self, record: &Value) -> Result<()> {
        // Readers must see either the previous snapshot or the complete new
        // one if a pane is terminated during a write. The lock serializes this
        // temporary path across viewers, actions, and hooks.
        let temporary = self.path.with_extension("tmp");
        fs::write(&temporary, serde_json::to_vec(record)?)?;
        fs::rename(temporary, &self.path)?;
        Ok(())
    }

    fn clear(&self) -> Result<()> {
        match fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }
}

// Only action-managed viewers participate. Direct pane opens and standalone
// --sidebar runs must not read or overwrite a workspace's shared view.
pub struct SidebarSession {
    workspace: String,
    pane: String,
    snapshot: Value,
    geometry_due: Instant,
}

impl SidebarSession {
    pub fn restore(app: &mut crate::app::App) -> Result<Option<Self>> {
        if env::var("HERDR_GIT_GRAPH_SHARED").as_deref() != Ok("1") {
            return Ok(None);
        }
        let workspace = env::var("HERDR_WORKSPACE_ID")?;
        let pane = env::var("HERDR_PANE_ID")?;
        let state = SidebarState::lock(&workspace)?;
        let Some(record) = state.read()? else {
            return Ok(None);
        };
        if record["pane_id"] != pane {
            return Ok(None);
        }
        super::view_state::restore(app, &record["view"]);
        Ok(Some(Self {
            workspace,
            pane,
            snapshot: Value::Null,
            geometry_due: Instant::now(),
        }))
    }

    pub fn update(&mut self, app: &crate::app::App, resized: bool) -> Result<()> {
        let snapshot = super::view_state::snapshot(app);
        // Headless Herdr can change logical split geometry without resizing
        // the PTY. Check occasionally as well as on terminal resize events.
        let geometry_due = resized || Instant::now() >= self.geometry_due;
        if self.snapshot == snapshot && !geometry_due {
            return Ok(());
        }
        let state = SidebarState::lock(&self.workspace)?;
        if let Some(mut record) = state.read()?
            && record["pane_id"] == self.pane
        {
            let previous = record.clone();
            record["view"] = snapshot.clone();
            if geometry_due {
                let bin = env::var_os("HERDR_BIN_PATH").unwrap_or_else(|| "herdr".into());
                if let Ok(actual) = pane_width(&bin, &self.pane) {
                    remember_width(&mut record, actual);
                }
            }
            if record != previous {
                state.write(&record)?;
            }
        }
        self.snapshot = snapshot;
        if geometry_due {
            self.geometry_due = Instant::now() + Duration::from_secs(1);
        }
        Ok(())
    }

    pub fn close(&self) -> Result<()> {
        let state = SidebarState::lock(&self.workspace)?;
        if let Some(record) = state.read()?
            && record["pane_id"] == self.pane
        {
            state.clear()?;
        }
        Ok(())
    }
}

fn string<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|v| !v.is_empty())
        .with_context(|| format!("Herdr 응답에 {key} 값이 없습니다."))
}

fn request(bin: &OsStr, args: &[&str]) -> Result<Value> {
    let output = Command::new(bin).args(args).output()?;
    ensure!(
        output.status.success(),
        "Herdr 명령을 실행하지 못했습니다: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    serde_json::from_slice(&output.stdout).context("Herdr 응답을 읽지 못했습니다.")
}

fn panes(bin: &OsStr, workspace: &str) -> Result<Vec<Value>> {
    let response = request(bin, &["pane", "list", "--workspace", workspace])?;
    response
        .pointer("/result/panes")
        .and_then(Value::as_array)
        .cloned()
        .context("Herdr 응답에 패널 목록이 없습니다.")
}

fn saved_pane(panes: &[Value], record: &Value) -> Option<Value> {
    panes
        .iter()
        .find(|pane| {
            pane["pane_id"].is_string()
                && pane["terminal_id"].is_string()
                && pane["pane_id"] == record["pane_id"]
                && pane["terminal_id"] == record["terminal_id"]
        })
        .cloned()
}

fn create_sidebar(
    state: &SidebarState,
    root: &Path,
    theme: ThemeMode,
    source: &str,
    previous: Option<&Value>,
) -> Result<Vec<u8>> {
    let mut record = previous.cloned().unwrap_or_else(|| json!({}));
    record["repo"] = json!(root);
    record["theme"] = json!(theme.to_possible_value().unwrap().get_name());
    let width = preferred_width(&record);
    record["width"] = json!(width);
    let output = open_view(root, Some((source, width)), theme)?;
    let response: Value = serde_json::from_slice(&output)?;
    let pane = response
        .pointer("/result/plugin_pane/pane")
        .context("새 사이드바 패널 정보가 없습니다.")?;
    let bin = env::var_os("HERDR_BIN_PATH").unwrap_or_else(|| "herdr".into());
    record["actual_width"] = json!(pane_width(&bin, string(pane, "pane_id")?)?);
    state.save(pane, &record)?;
    Ok(output)
}

pub fn open_sidebar(explicit: Option<&Path>, theme: ThemeMode) -> Result<()> {
    sidebar_action(explicit, theme, false)
}

pub fn toggle_sidebar(explicit: Option<&Path>, theme: ThemeMode) -> Result<()> {
    sidebar_action(explicit, theme, true)
}

fn sidebar_action(explicit: Option<&Path>, theme: ThemeMode, toggle: bool) -> Result<()> {
    let context = plugin_context();
    let source = context_id(&context, "focused_pane_id", "HERDR_PANE_ID")
        .context("사이드바를 열 대상 패널이 없습니다. Herdr 패널 안에서 액션을 실행하세요.")?;
    let workspace = context_id(&context, "workspace_id", "HERDR_WORKSPACE_ID")
        .context("사이드바를 열 작업 공간이 없습니다.")?;
    let state = SidebarState::lock(&workspace)?;
    let bin = env::var_os("HERDR_BIN_PATH").unwrap_or_else(|| "herdr".into());
    if let Some(record) = state.read()? {
        let panes = panes(&bin, &workspace)?;
        if let Some(pane) = saved_pane(&panes, &record) {
            if toggle {
                let closed = request(&bin, &["pane", "close", string(&pane, "pane_id")?])?;
                // Keep the lock through close and opt-out so tab/pane hooks
                // cannot recreate a sidebar the user has just toggled off.
                state.clear()?;
                serde_json::to_writer(std::io::stdout(), &closed)?;
                return Ok(());
            }
            let target = panes
                .iter()
                .find(|pane| pane["pane_id"] == source)
                .context("사이드바를 열 대상 패널이 없습니다.")?;
            let pane = relocate_sidebar(&state, &bin, &pane, target, &record)?;
            serde_json::to_writer(
                std::io::stdout(),
                &json!({
                    "result": {"plugin_pane": {
                        "plugin_id": "herdr.git-graph", "entrypoint": "sidebar", "pane": pane
                    }}
                }),
            )?;
            return Ok(());
        }
        state.clear()?;
    }
    let root = super::repository_path(explicit)?;
    let output = create_sidebar(&state, &root, theme, &source, None)?;
    std::io::stdout().write_all(&output)?;
    Ok(())
}

// Hooks use live focus under the lock: event contexts may already be stale,
// and the focused destination need not be a Git directory.
pub fn sync_sidebar() -> Result<()> {
    let context = plugin_context();
    let event: Value = env::var("HERDR_PLUGIN_EVENT_JSON")
        .ok()
        .and_then(|value| serde_json::from_str(&value).ok())
        .unwrap_or(Value::Null);
    let Some(workspace) = event["data"]["workspace_id"]
        .as_str()
        .map(str::to_owned)
        .or_else(|| context_id(&context, "workspace_id", "HERDR_WORKSPACE_ID"))
    else {
        return Ok(());
    };
    let state = SidebarState::lock(&workspace)?;
    let Some(record) = state.read()? else {
        return Ok(());
    };
    if matches!(
        env::var("HERDR_PLUGIN_EVENT").as_deref(),
        Ok("pane.closed" | "pane.exited")
    ) {
        // Herdr emits these for a directly closed/exited pane, not for each
        // child of a deleted tab. Delayed events must not disable a replacement.
        if event["data"]["pane_id"] == record["pane_id"] {
            state.clear()?;
        }
        return Ok(());
    }
    let bin = env::var_os("HERDR_BIN_PATH").unwrap_or_else(|| "herdr".into());
    let response = request(&bin, &["workspace", "list"])?;
    let workspaces = response
        .pointer("/result/workspaces")
        .and_then(Value::as_array)
        .context("Herdr 응답에 작업 공간 목록이 없습니다.")?;
    let Some(workspace_info) = workspaces
        .iter()
        .find(|info| info["workspace_id"] == workspace)
    else {
        // Closing the last tab removes its workspace; never restore elsewhere.
        return state.clear();
    };
    if workspace_info["focused"] != true {
        return Ok(());
    }
    let tab = string(workspace_info, "active_tab_id")?;
    let panes = panes(&bin, &workspace)?;
    let target = panes
        .iter()
        .find(|pane| pane["tab_id"] == tab && pane["focused"] == true)
        .or_else(|| panes.iter().find(|pane| pane["tab_id"] == tab));
    if let Some(pane) = saved_pane(&panes, &record) {
        if pane["tab_id"] == tab {
            return Ok(());
        }
        if let Some(target) = target {
            relocate_sidebar(&state, &bin, &pane, target, &record)?;
        }
        return Ok(());
    }
    // Legacy records lack recovery settings. A reused pane ID must also never
    // be treated as permission to reopen an old sidebar after a server restart.
    let (Some(previous_tab), Some(repo), Some(theme)) = (
        record["tab_id"].as_str(),
        record["repo"].as_str(),
        record["theme"].as_str(),
    ) else {
        return state.clear();
    };
    if panes
        .iter()
        .any(|pane| pane["pane_id"] == record["pane_id"])
    {
        return state.clear();
    }
    let response = request(&bin, &["tab", "list", "--workspace", &workspace])?;
    let tabs = response
        .pointer("/result/tabs")
        .and_then(Value::as_array)
        .context("Herdr 응답에 탭 목록이 없습니다.")?;
    if tabs.iter().any(|tab| tab["tab_id"] == previous_tab) {
        // Its tab survived, so only the sidebar was closed: opt out.
        return state.clear();
    }
    if let Some(target) = target {
        let theme = ThemeMode::from_str(theme, false).map_err(anyhow::Error::msg)?;
        create_sidebar(
            &state,
            Path::new(repo),
            theme,
            string(target, "pane_id")?,
            Some(&record),
        )?;
    }
    Ok(())
}

fn pane_width(bin: &OsStr, pane: &str) -> Result<u16> {
    let layout = request(bin, &["pane", "layout", "--pane", pane])?;
    let width = layout
        .pointer("/result/layout/panes")
        .and_then(Value::as_array)
        .and_then(|panes| panes.iter().find(|p| p["pane_id"] == pane))
        .and_then(|p| p.pointer("/rect/width"))
        .and_then(Value::as_u64)
        .and_then(|w| u16::try_from(w).ok())
        .filter(|w| *w > 0)
        .context("사이드바를 옮길 패널의 너비를 확인하지 못했습니다.")?;
    Ok(width)
}

fn preferred_width(record: &Value) -> u16 {
    record["width"]
        .as_u64()
        .and_then(|width| u16::try_from(width).ok())
        .filter(|width| *width > 0)
        .unwrap_or(SIDEBAR_WIDTH)
}

fn remember_width(record: &mut Value, actual: u16) {
    // A move records its constrained actual width before releasing the lock.
    // A later difference is a user/host resize, not a temporary move clamp.
    if record["width"].is_null()
        || record["actual_width"]
            .as_u64()
            .is_some_and(|width| width != u64::from(actual))
    {
        record["width"] = json!(actual);
    }
    record["actual_width"] = json!(actual);
}

fn relocate_sidebar(
    state: &SidebarState,
    bin: &OsStr,
    pane: &Value,
    target: &Value,
    previous: &Value,
) -> Result<Value> {
    let mut record = previous.clone();
    remember_width(&mut record, pane_width(bin, string(pane, "pane_id")?)?);
    let moved = move_sidebar(bin, pane, target, preferred_width(&record))?;
    record["actual_width"] = json!(pane_width(bin, string(&moved, "pane_id")?)?);
    state.save(&moved, &record)?;
    Ok(moved)
}

fn move_sidebar(bin: &OsStr, pane: &Value, target: &Value, width: u16) -> Result<Value> {
    if pane["tab_id"] == target["tab_id"] {
        return Ok(pane.clone());
    }
    let pane_id = string(pane, "pane_id")?;
    let target_id = string(target, "pane_id")?;
    let tab = string(target, "tab_id")?;
    let available = pane_width(bin, target_id)?;
    let ratio = (1.0 - f64::from(width) / f64::from(available)).clamp(0.1, 0.9);
    let response = request(
        bin,
        &[
            "pane",
            "move",
            pane_id,
            "--tab",
            tab,
            "--split",
            "right",
            "--target-pane",
            target_id,
            "--ratio",
            &ratio.to_string(),
            "--no-focus",
        ],
    )?;
    let moved = response
        .pointer("/result/move_result/pane")
        .context("이동한 사이드바 패널 정보가 없습니다.")?;
    ensure!(
        moved["tab_id"] == tab,
        "사이드바를 옮기지 못했습니다. 출발·대상 탭의 패널 확대를 해제한 뒤 탭을 전환하거나 사이드바 열기를 다시 실행하세요."
    );
    Ok(moved.clone())
}
