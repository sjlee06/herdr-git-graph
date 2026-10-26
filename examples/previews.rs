// Rebuild every documentation preview from the built-in demo history and patch.
// Usage: cargo run --example previews -- [output-directory] (default: docs).
use anyhow::{Context, Result, ensure};
use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use herdr_git_graph::{
    app::{App, Focus},
    git,
    theme::{Theme, ThemeMode},
    ui,
};
use std::{env, fs, path::Path, path::PathBuf};

const LIMIT: usize = 2000;

fn main() -> Result<()> {
    let output = env::args_os()
        .nth(1)
        .map_or_else(|| PathBuf::from("docs"), PathBuf::from);
    fs::create_dir_all(&output)?;

    for (suffix, theme) in [
        ("dark", Theme::new(ThemeMode::Classic)),
        ("light", tomorrow()),
    ] {
        let mut full = App::new(git::demo(), LIMIT, true);
        full.theme = theme;
        // The inspector is short: start at the graph patch, not the commit metadata.
        full.detail_scroll = full
            .detail
            .lines()
            .position(|line| line == "diff --git a/src/graph.rs b/src/graph.rs")
            .context("graph patch in demo detail")?
            + 1; // The UI prepends a refs row.
        full.focus = Focus::Details;
        ensure!(full.selected == Some(0) && full.show_details && !full.sidebar);
        let filename = if suffix == "dark" {
            "preview.svg"
        } else {
            "preview-light.svg"
        };
        snapshot(&mut full, &output, filename, 140, 56, true)?;
        let visible = full
            .detail
            .lines()
            .skip(full.detail_scroll - 1)
            .take(usize::from(full.detail_area.height))
            .collect::<Vec<_>>();
        ensure!(
            visible
                .iter()
                .any(|line| line.starts_with('-') && !line.starts_with("---"))
        );
        ensure!(
            visible
                .iter()
                .any(|line| line.starts_with('+') && !line.starts_with("+++"))
        );
        if suffix == "dark" {
            snapshot(&mut full, &output, "preview-text.svg", 140, 56, false)?;
        }

        let mut sidebar = App::new(git::demo(), LIMIT, true).with_sidebar(true);
        sidebar.theme = theme;
        ensure!(sidebar.selected.is_none() && sidebar.files.is_none() && !sidebar.show_details);
        if suffix == "dark" {
            snapshot(&mut sidebar, &output, "workspace-content.svg", 38, 39, true)?;
        }
        snapshot(
            &mut sidebar,
            &output,
            &format!("preview-sidebar-{suffix}.svg"),
            40,
            26,
            true,
        )?;

        // Exercise the same mouse path as clicking the demo commit and its file.
        let commit_row = sidebar
            .history_rows
            .iter()
            .position(|index| *index == Some(0))
            .context("visible demo commit")?;
        click_history_row(&mut sidebar, commit_row);
        ensure!(sidebar.selected_oid() == full.selected_oid() && sidebar.file_diff.is_none());
        let files = sidebar
            .files
            .as_ref()
            .context("demo files")?
            .as_ref()
            .map_err(|error| anyhow::anyhow!("{error}"))?;
        ensure!(
            files
                .iter()
                .map(|file| (file.status.as_str(), file.path.as_str()))
                .collect::<Vec<_>>()
                == [
                    ("A", "src/curves.rs"),
                    ("M", "src/graph.rs"),
                    ("D", "src/legacy_graph.rs"),
                ]
        );
        let graph_index = files
            .iter()
            .position(|file| file.path == "src/graph.rs")
            .context("demo graph file")?;
        snapshot(
            &mut sidebar,
            &output,
            &format!("preview-sidebar-files-{suffix}.svg"),
            40,
            26,
            true,
        )?;
        ensure!(sidebar.top == 0 && sidebar.top_offset == 0);

        let file_row = sidebar
            .history_file_rows
            .iter()
            .position(|index| *index == Some(graph_index))
            .context("visible demo file")?;
        click_history_row(&mut sidebar, file_row);
        ensure!(sidebar.focus == Focus::Details && sidebar.show_details);
        ensure!(
            sidebar
                .file_diff
                .as_ref()
                .is_some_and(|file| file.path == "src/graph.rs")
        );
        ensure!(
            full.detail
                .contains(sidebar.detail.split("\n[Demo data").next().unwrap())
        );
        ensure!(
            !sidebar.detail.contains("src/curves.rs")
                && !sidebar.detail.contains("src/legacy_graph.rs")
        );
        ensure!(
            sidebar.detail.contains("-    draw_ascii_edge")
                && sidebar.detail.contains("+    let curve = BranchCurve::new")
        );
        snapshot(
            &mut sidebar,
            &output,
            &format!("preview-sidebar-diff-{suffix}.svg"),
            40,
            26,
            true,
        )?;
        ensure!(sidebar.detail.lines().count() <= usize::from(sidebar.detail_area.height - 2));
    }
    Ok(())
}

fn click_history_row(app: &mut App, row: usize) {
    app.mouse(
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: app.history_area.x,
            row: app.history_area.y + row as u16,
            modifiers: KeyModifiers::NONE,
        },
        None,
    );
}

fn snapshot(
    app: &mut App,
    output: &Path,
    name: &str,
    width: u16,
    height: u16,
    curves: bool,
) -> Result<()> {
    app.renderer_status = if curves { "CURVES" } else { "TEXT" }.into();
    let path = output.join(name);
    ui::snapshot(app, &path, width, height, curves)?;
    println!("Screen snapshot: {}", path.display());
    Ok(())
}

fn tomorrow() -> Theme {
    // Ghostty's Tomorrow palette, as returned to --theme auto by a light terminal.
    Theme {
        mode: ThemeMode::Auto,
        foreground: Some((77, 77, 76)),
        background: Some((255, 255, 255)),
        palette: [
            (200, 40, 41),
            (113, 140, 0),
            (234, 183, 0),
            (66, 113, 174),
            (137, 89, 168),
            (62, 153, 159),
        ]
        .map(Some),
    }
}
