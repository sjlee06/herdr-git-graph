mod layout;
mod sidebar;
mod view_state;

pub use sidebar::{SidebarSession, open_sidebar, sync_sidebar, toggle_sidebar};

use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::{
    env,
    ffi::OsStr,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

pub fn repository_path(explicit: Option<&Path>) -> Result<PathBuf> {
    if let Some(path) = explicit {
        return crate::git::discover(path);
    }
    if let Some(path) = env::var_os("HERDR_GIT_GRAPH_REPO") {
        return crate::git::discover(Path::new(&path));
    }
    let cwd = env::current_dir()?;
    let plugin_root = env::var_os("HERDR_PLUGIN_ROOT").map(PathBuf::from);
    // A pane opened with --cwd takes precedence over the source pane's context.
    if plugin_root.as_ref() != Some(&cwd)
        && let Ok(root) = crate::git::discover(&cwd)
    {
        return Ok(root);
    }
    let context = plugin_context();
    for path in context_paths(&context) {
        if let Ok(root) = crate::git::discover(&path) {
            return Ok(root);
        }
    }
    crate::git::discover(&cwd)
}

fn plugin_context() -> Value {
    env::var("HERDR_PLUGIN_CONTEXT_JSON")
        .ok()
        .and_then(|v| serde_json::from_str(&v).ok())
        .unwrap_or(Value::Null)
}

fn context_id(context: &Value, key: &str, fallback_env: &str) -> Option<String> {
    context
        .get(key)
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
        .or_else(|| env::var(fallback_env).ok().filter(|id| !id.is_empty()))
}

pub fn context_paths(context: &Value) -> Vec<PathBuf> {
    [
        context.get("focused_pane_cwd"),
        context.pointer("/worktree/checkout_path"),
        context.get("workspace_cwd"),
    ]
    .into_iter()
    .flatten()
    .filter_map(Value::as_str)
    .filter(|s| !s.is_empty())
    .map(PathBuf::from)
    .collect()
}

pub fn open_pane(root: &Path, theme: crate::theme::ThemeMode) -> Result<()> {
    std::io::stdout().write_all(&open_view(root, None, theme)?)?;
    Ok(())
}

fn open_view(
    root: &Path,
    source_pane: Option<&str>,
    theme: crate::theme::ThemeMode,
) -> Result<Vec<u8>> {
    use clap::ValueEnum;
    let sidebar = source_pane.is_some();
    let context = plugin_context();
    let bin = env::var_os("HERDR_BIN_PATH").unwrap_or_else(|| "herdr".into());
    let mut command = Command::new(&bin);
    command
        .args([
            "plugin",
            "pane",
            "open",
            "--plugin",
            "herdr.git-graph",
            "--entrypoint",
            if sidebar { "sidebar" } else { "graph" },
            "--placement",
            if sidebar { "split" } else { "tab" },
            if sidebar { "--no-focus" } else { "--focus" },
            "--cwd",
        ])
        .arg(root)
        .arg("--env")
        .arg(format!("HERDR_GIT_GRAPH_REPO={}", root.display()))
        .arg("--env")
        .arg(format!(
            "HERDR_GIT_GRAPH_THEME={}",
            theme.to_possible_value().unwrap().get_name()
        ));
    if let Some(id) = source_pane {
        command.args(["--env", "HERDR_GIT_GRAPH_SHARED=1"]);
        // Split placement accepts a target pane, but rejects workspace_id.
        // Recovery uses live focus, not the closed tab's invocation context.
        command
            .args(["--direction", "right", "--target-pane"])
            .arg(id);
    } else if let Some(id) = context_id(&context, "workspace_id", "HERDR_WORKSPACE_ID") {
        command.arg("--workspace").arg(id);
    }
    let output = command
        .stderr(Stdio::inherit())
        .output()
        .context("Herdr를 실행할 수 없습니다.")?;
    if !output.status.success() {
        bail!("Herdr Git Graph 패널을 열지 못했습니다. 플러그인 등록을 확인하세요.");
    }
    Ok(output.stdout)
}

// Allow room for Herdr borders/scrollbar, graph lanes, and the full
// "Uncommitted changes" subject. This is the outer split width in columns.
const SIDEBAR_WIDTH: u16 = 40;

fn size_sidebar(bin: &OsStr, source: &str, pane: &str, width: u16) -> Result<()> {
    let output = Command::new(bin)
        .args(["pane", "layout", "--pane", pane])
        .output()?;
    anyhow::ensure!(
        output.status.success(),
        "패널 레이아웃을 조회하지 못했습니다."
    );
    let response: Value = serde_json::from_slice(&output.stdout)?;
    let layout = response
        .pointer("/result/layout")
        .context("패널 레이아웃이 없습니다.")?;
    let (direction, amount) = sidebar_resize(layout, source, pane, width)
        .context("사이드바 분할 크기를 찾지 못했습니다.")?;
    if amount < f64::EPSILON {
        return Ok(());
    }
    // Use the pane facing the new divider so a nested split cannot select an
    // existing ancestor divider on the other side of the source pane.
    let target = if direction == "right" { source } else { pane };
    let output = Command::new(bin)
        .args([
            "pane",
            "resize",
            "--pane",
            target,
            "--direction",
            direction,
            "--amount",
        ])
        .arg(amount.to_string())
        .output()?;
    anyhow::ensure!(
        output.status.success(),
        "사이드바 분할 크기를 조절하지 못했습니다."
    );
    Ok(())
}

fn sidebar_resize(
    layout: &Value,
    source: &str,
    pane: &str,
    width: u16,
) -> Option<(&'static str, f64)> {
    use ratatui::layout::Rect;
    fn rect(value: &Value) -> Option<Rect> {
        let field = |key| u16::try_from(value.get(key)?.as_u64()?).ok();
        Some(Rect::new(
            field("x")?,
            field("y")?,
            field("width")?,
            field("height")?,
        ))
    }
    let panes = layout.get("panes")?.as_array()?;
    let find = |id: &str| rect(panes.iter().find(|p| p["pane_id"] == id)?.get("rect")?);
    let source_rect = find(source)?;
    let pane_rect = find(pane)?;
    let (split, area) = layout
        .get("splits")?
        .as_array()?
        .iter()
        .filter(|split| split["direction"] == "right")
        .filter_map(|split| Some((split, rect(split.get("rect")?)?)))
        .filter(|(_, area)| {
            area.width > 0
                && area.intersection(source_rect) == source_rect
                && area.intersection(pane_rect) == pane_rect
        })
        .min_by_key(|(_, area)| u32::from(area.width) * u32::from(area.height))?;
    let current = split.get("ratio")?.as_f64()?;
    if !current.is_finite() || !(0.1..=0.9).contains(&current) {
        return None;
    }
    // Keep the requested column width, subject only to Herdr's split limits.
    let target = (1.0 - f64::from(width) / f64::from(area.width)).clamp(0.1, 0.9);
    Some((
        if target >= current { "right" } else { "left" },
        (target - current).abs(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sidebar_width_uses_its_own_split_and_respects_herdr_limits() {
        for (total, current, expected) in [
            (100, 0.5, 0.6),
            (80, 0.8, 0.5),
            (40, 0.5, 0.1),
            (400, 0.5, 0.9),
        ] {
            let first = (f64::from(total) * current).round() as u16;
            let layout = json!({
                "panes": [
                    {"pane_id":"source", "rect":{"x":30,"y":2,"width":first,"height":20}},
                    {"pane_id":"graph", "rect":{"x":30+first,"y":2,"width":total-first,"height":20}}
                ],
                "splits": [
                    {"direction":"right","ratio":0.5,"rect":{"x":0,"y":0,"width":500,"height":40}},
                    {"direction":"down","ratio":0.5,"rect":{"x":30,"y":0,"width":total,"height":40}},
                    {"direction":"right","ratio":current,"rect":{"x":30,"y":2,"width":total,"height":20}}
                ]
            });
            let (direction, amount) = sidebar_resize(&layout, "source", "graph", 40).unwrap();
            let ratio = current
                + if direction == "right" {
                    amount
                } else {
                    -amount
                };
            assert!((ratio - expected).abs() < 0.0001, "{total}: {ratio}");
            assert!(sidebar_resize(&layout, "missing", "graph", 40).is_none());
        }
        assert!(sidebar_resize(&json!({}), "source", "graph", 40).is_none());
    }

    #[test]
    fn worktree_context_uses_checkout_not_shared_repo_root() {
        let paths = context_paths(
            &json!({"focused_pane_cwd":"/repo/sub dir", "worktree":{"checkout_path":"/worktrees/feature", "repo_root":"/repo"}, "workspace_cwd":"/workspace"}),
        );
        assert_eq!(
            paths,
            vec![
                PathBuf::from("/repo/sub dir"),
                PathBuf::from("/worktrees/feature"),
                PathBuf::from("/workspace")
            ]
        );
    }
}
