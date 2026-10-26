// Rebuild every documentation preview from a deterministic, real Git repository.
// Usage: cargo run --example previews -- [output-directory] (default: docs).
use anyhow::{Context, Result, ensure};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use herdr_git_graph::{
    app::{App, Focus, Request, Response, Worker},
    git,
    theme::{Theme, ThemeMode},
    ui,
};
use std::{env, fs, path::Path, path::PathBuf, process::Command, time::Duration};

const LIMIT: usize = 2000;
const GRAPH_BEFORE: &str =
    "pub fn node(n: usize) -> String {\n    format!(\"{}*\", \" \".repeat(n))\n}\n";
const GRAPH_AFTER: &str = "pub fn node(n: usize) -> String {\n    let indent = \"  \".repeat(n);\n    format!(\"{indent}●\")\n}\n";

fn main() -> Result<()> {
    // Re-exec with an isolated Git environment before starting any worker threads.
    // This also makes the library's Git subprocesses ignore the user's config.
    let temporary = tempfile::tempdir()?;
    if env::var_os("HERDR_PREVIEWS_ISOLATED").is_none() {
        let config = temporary.path().join("gitconfig");
        fs::write(&config, "")?;
        let mut command = Command::new(env::current_exe()?);
        command.args(env::args_os().skip(1));
        for (key, _) in env::vars_os() {
            if key.to_string_lossy().starts_with("GIT_") {
                command.env_remove(key);
            }
        }
        let status = command
            .env("HERDR_PREVIEWS_ISOLATED", "1")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", config)
            .env("GIT_ATTR_NOSYSTEM", "1")
            .env("HOME", temporary.path())
            .env("XDG_CONFIG_HOME", temporary.path())
            .env("LC_ALL", "C")
            .env("TZ", "UTC")
            .status()?;
        ensure!(status.success(), "preview generator failed: {status}");
        return Ok(());
    }

    let output = env::args_os()
        .nth(1)
        .map_or_else(|| PathBuf::from("docs"), PathBuf::from);
    fs::create_dir_all(&output)?;
    let root = sample_repository(temporary.path())?;
    let repo = git::load(&root, None, LIMIT)?;
    ensure!(repo.commits.len() == 10 && repo.worktree.files.is_empty());
    ensure!(repo.commits[0].parents.len() == 2);
    ensure!(repo.commits[0].refs.contains("origin/main"));
    ensure!(
        repo.commits
            .iter()
            .any(|commit| commit.refs.contains("v0.4.0"))
    );
    let worker = Worker::start(root, LIMIT);

    for (suffix, theme) in [
        ("dark", Theme::new(ThemeMode::Classic)),
        ("light", tomorrow()),
    ] {
        let mut full = App::new(repo.clone(), LIMIT, false);
        full.theme = theme;
        worker.sender.send(Request::Detail {
            oid: full.selected_oid().context("selected commit")?.into(),
            id: full.load_id,
        })?;
        let response = worker.receiver.recv_timeout(Duration::from_secs(20))?;
        ensure!(matches!(&response, Response::Detail { result: Ok(_), .. }));
        ensure!(full.apply(response));
        // The inspector is short: start at the graph patch, not the commit metadata.
        full.detail_scroll = full
            .detail
            .lines()
            .position(|line| line == "diff --git a/src/graph.rs b/src/graph.rs")
            .context("graph patch in merge detail")?
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

        let mut sidebar = App::new(repo.clone(), LIMIT, false).with_sidebar(true);
        sidebar.theme = theme;
        ensure!(sidebar.selected.is_none() && sidebar.files.is_none() && !sidebar.show_details);
        snapshot(
            &mut sidebar,
            &output,
            &format!("preview-sidebar-{suffix}.svg"),
            40,
            26,
            true,
        )?;
        if suffix == "dark" {
            snapshot(&mut sidebar, &output, "workspace-content.svg", 38, 39, true)?;
        }

        sidebar.key(
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            Some(&worker),
        );
        ensure!(sidebar.selected == Some(0) && sidebar.file_diff.is_none());
        worker.sender.send(Request::Files {
            oid: sidebar.selected_oid().context("expanded commit")?.into(),
            id: sidebar.load_id,
        })?;
        let response = worker.receiver.recv_timeout(Duration::from_secs(20))?;
        ensure!(matches!(&response, Response::Files { result: Ok(_), .. }));
        ensure!(sidebar.apply(response));
        let files = sidebar
            .files
            .as_ref()
            .context("loaded files")?
            .as_ref()
            .map_err(|error| anyhow::anyhow!("{error}"))?;
        ensure!(
            files
                .iter()
                .map(|file| (file.path.as_str(), file.status.as_str()))
                .collect::<Vec<_>>()
                == [
                    ("docs/usage.md", "M"),
                    ("src/graph.rs", "M"),
                    ("src/inspector.rs", "A"),
                ]
        );
        let graph_index = files
            .iter()
            .position(|file| file.path == "src/graph.rs")
            .unwrap();
        snapshot(
            &mut sidebar,
            &output,
            &format!("preview-sidebar-files-{suffix}.svg"),
            40,
            26,
            true,
        )?;

        sidebar.open_file(graph_index);
        sidebar.request_detail(&worker);
        let response = worker.receiver.recv_timeout(Duration::from_secs(20))?;
        ensure!(matches!(
            &response,
            Response::FileDetail { result: Ok(_), .. }
        ));
        ensure!(sidebar.apply(response));
        ensure!(sidebar.focus == Focus::Details && sidebar.show_details);
        ensure!(
            sidebar
                .file_diff
                .as_ref()
                .is_some_and(|file| file.path == "src/graph.rs")
        );
        ensure!(
            sidebar.detail.contains("-    format!") && sidebar.detail.contains("+    let indent")
        );
        ensure!(
            !sidebar.detail.contains("docs/usage.md")
                && !sidebar.detail.contains("src/inspector.rs")
        );
        ensure!(sidebar.detail.lines().count() <= 20);
        snapshot(
            &mut sidebar,
            &output,
            &format!("preview-sidebar-diff-{suffix}.svg"),
            40,
            26,
            true,
        )?;
        ensure!(sidebar.detail.lines().count() <= usize::from(sidebar.detail_area.height));
    }
    Ok(())
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

struct Sample {
    root: PathBuf,
    day: u8,
}

impl Sample {
    fn git(&self, args: &[&str]) -> Result<String> {
        let date = format!("2026-10-{:02}T12:00:00+0000", self.day);
        let result = Command::new("git")
            .arg("-C")
            .arg(&self.root)
            .args(args)
            .env("GIT_AUTHOR_NAME", "Alex Kim")
            .env("GIT_AUTHOR_EMAIL", "alex@example.invalid")
            .env("GIT_COMMITTER_NAME", "Alex Kim")
            .env("GIT_COMMITTER_EMAIL", "alex@example.invalid")
            .env("GIT_AUTHOR_DATE", &date)
            .env("GIT_COMMITTER_DATE", &date)
            .output()
            .context("run Git (Git must be on PATH)")?;
        ensure!(
            result.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        Ok(String::from_utf8(result.stdout)?.trim().into())
    }

    fn write(&self, path: &str, content: &str) -> Result<()> {
        let path = self.root.join(path);
        fs::create_dir_all(path.parent().context("sample file parent")?)?;
        fs::write(path, content)?;
        Ok(())
    }

    fn commit(&mut self, message: &str) -> Result<()> {
        self.git(&["add", "--all"])?;
        self.git(&["commit", "-m", message])?;
        self.day += 1;
        Ok(())
    }

    fn merge(&mut self, branch: &str, message: &str) -> Result<()> {
        self.git(&["merge", "--no-ff", branch, "-m", message])?;
        self.day += 1;
        Ok(())
    }
}

fn sample_repository(parent: &Path) -> Result<PathBuf> {
    let mut sample = Sample {
        root: parent.join("herdr-git-graph"),
        day: 1,
    };
    fs::create_dir(&sample.root)?;
    // Disable templates/hooks and force SHA-1 even if the user's Git defaults differ.
    sample.git(&["init", "--template=", "--object-format=sha1", "-b", "main"])?;
    for (key, value) in [
        ("core.autocrlf", "false"),
        ("core.abbrev", "7"),
        ("commit.gpgsign", "false"),
        ("tag.gpgsign", "false"),
    ] {
        sample.git(&["config", key, value])?;
    }
    sample.write("src/graph.rs", GRAPH_BEFORE)?;
    sample.write(
        "src/main.rs",
        "mod graph;\n\nfn main() {\n    println!(\"{}\", graph::node(0));\n}\n",
    )?;
    sample.commit("Initialize Git graph viewer")?;
    sample.write(
        "docs/usage.md",
        "# Git Graph\n\nUse Up and Down to select a commit.\n",
    )?;
    sample.commit("Document history navigation")?;
    sample.write(
        "src/theme.rs",
        "pub const LANES: [&str; 3] = [\"cyan\", \"blue\", \"green\"];\n",
    )?;
    sample.commit("Add branch lane colors")?;
    sample.git(&["tag", "v0.4.0"])?;
    sample.git(&["checkout", "-b", "feature/sidebar"])?;
    sample.write("src/sidebar.rs", "pub fn title(repository: &str, branch: &str) -> String {\n    format!(\"{repository} / {branch}\")\n}\n")?;
    sample.commit("Add compact history sidebar")?;
    sample.git(&["checkout", "main"])?;
    sample.write(
        "src/theme.rs",
        "pub const LANES: [&str; 4] = [\"cyan\", \"blue\", \"green\", \"magenta\"];\n",
    )?;
    sample.commit("Polish branch lane colors")?;
    sample.git(&["checkout", "-b", "feature/file-diff"])?;
    sample.write("src/graph.rs", GRAPH_AFTER)?;
    sample.write("src/inspector.rs", "pub fn changed_lines(patch: &str) -> usize {\n    patch.lines().filter(|line| line.starts_with('+') && !line.starts_with(\"+++\")).count()\n}\n")?;
    sample.commit("Load selected file patches")?;
    sample.write("docs/usage.md", "# Git Graph\n\nUse Up and Down to select a commit.\nPress Enter to expand files, then Enter to inspect a patch.\nPress Esc to return to history.\n")?;
    sample.commit("Document file inspection")?;
    sample.git(&["checkout", "main"])?;
    sample.write("src/main.rs", "mod graph;\n\nfn main() {\n    for lane in 0..3 {\n        println!(\"{}\", graph::node(lane));\n    }\n}\n")?;
    sample.commit("Render every branch lane")?;
    sample.merge("feature/sidebar", "Merge compact sidebar")?;
    sample.merge("feature/file-diff", "Merge file inspection")?;
    sample.git(&["update-ref", "refs/remotes/origin/main", "HEAD"])?;
    Ok(sample.root)
}
