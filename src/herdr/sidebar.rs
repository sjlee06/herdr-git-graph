use super::{SIDEBAR_WIDTH, context_id, open_view, plugin_context};
use anyhow::{Context, Result, ensure};
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

    fn save(&self, pane: &Value) -> Result<()> {
        // Terminal identity prevents a stale record attaching to a new process
        // after a server restart reuses a public pane ID.
        let record = json!({
            "pane_id": string(pane, "pane_id")?,
            "terminal_id": string(pane, "terminal_id")?,
        });
        fs::write(&self.path, serde_json::to_vec(&record)?)?;
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

fn saved_pane(state: &SidebarState, panes: &[Value], record: &Value) -> Result<Option<Value>> {
    let pane = panes.iter().find(|pane| {
        pane["pane_id"].is_string()
            && pane["terminal_id"].is_string()
            && pane["pane_id"] == record["pane_id"]
            && pane["terminal_id"] == record["terminal_id"]
    });
    if pane.is_none() {
        // Closing the sidebar opts out until the next explicit open action.
        state.clear()?;
    }
    Ok(pane.cloned())
}

pub fn open_sidebar(explicit: Option<&Path>, theme: crate::theme::ThemeMode) -> Result<()> {
    let context = plugin_context();
    let source = context_id(&context, "focused_pane_id", "HERDR_PANE_ID")
        .context("사이드바를 열 대상 패널이 없습니다. Herdr 패널 안에서 액션을 실행하세요.")?;
    let workspace = context_id(&context, "workspace_id", "HERDR_WORKSPACE_ID")
        .context("사이드바를 열 작업 공간이 없습니다.")?;
    let state = SidebarState::lock(&workspace)?;
    let bin = env::var_os("HERDR_BIN_PATH").unwrap_or_else(|| "herdr".into());
    if let Some(record) = state.read()? {
        let panes = panes(&bin, &workspace)?;
        if let Some(pane) = saved_pane(&state, &panes, &record)? {
            let target = panes
                .iter()
                .find(|pane| pane["pane_id"] == source)
                .context("사이드바를 열 대상 패널이 없습니다.")?;
            let pane = move_sidebar(&bin, &pane, target)?;
            state.save(&pane)?;
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
    }
    let root = super::repository_path(explicit)?;
    let output = open_view(&root, true, theme)?;
    let response: Value = serde_json::from_slice(&output)?;
    let pane = response
        .pointer("/result/plugin_pane/pane")
        .context("새 사이드바 패널 정보가 없습니다.")?;
    state.save(pane)?;
    std::io::stdout().write_all(&output)?;
    Ok(())
}

// Event hooks must not discover a repository: a destination tab can be a shell
// in a non-Git directory. Read live focus under the lock rather than trusting
// the event's tab ID, which can already be stale during rapid tab switching.
pub fn sync_sidebar() -> Result<()> {
    let context = plugin_context();
    let Some(workspace) = context_id(&context, "workspace_id", "HERDR_WORKSPACE_ID") else {
        return Ok(());
    };
    let state = SidebarState::lock(&workspace)?;
    let Some(record) = state.read()? else {
        return Ok(());
    };
    let bin = env::var_os("HERDR_BIN_PATH").unwrap_or_else(|| "herdr".into());
    let response = request(&bin, &["workspace", "get", &workspace])?;
    let workspace_info = &response["result"]["workspace"];
    if workspace_info["focused"] != true {
        return Ok(());
    }
    let tab = string(workspace_info, "active_tab_id")?;
    let panes = panes(&bin, &workspace)?;
    let Some(pane) = saved_pane(&state, &panes, &record)? else {
        return Ok(());
    };
    if pane["tab_id"] == tab {
        return Ok(());
    }
    let target = panes
        .iter()
        .find(|pane| pane["tab_id"] == tab && pane["focused"] == true)
        .or_else(|| panes.iter().find(|pane| pane["tab_id"] == tab));
    if let Some(target) = target {
        let moved = move_sidebar(&bin, &pane, target)?;
        state.save(&moved)?;
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

fn move_sidebar(bin: &OsStr, pane: &Value, target: &Value) -> Result<Value> {
    if pane["tab_id"] == target["tab_id"] {
        return Ok(pane.clone());
    }
    let pane_id = string(pane, "pane_id")?;
    let target_id = string(target, "pane_id")?;
    let tab = string(target, "tab_id")?;
    let width = pane_width(bin, pane_id).unwrap_or(SIDEBAR_WIDTH);
    let available = pane_width(bin, target_id)?;
    let width = width.min(available / 2);
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
