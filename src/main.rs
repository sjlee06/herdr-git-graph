use anyhow::{Context, Result};
use clap::{Parser, ValueEnum};
use crossterm::event::{self, Event, KeyEventKind};
use herdr_git_graph::{
    app::{App, Worker},
    git,
    graphics::{Surface, Viewport},
    herdr,
    theme::{Theme, ThemeMode},
    theme_probe::ThemeProbe,
    ui,
};
use std::{
    io::{self, IsTerminal},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

mod terminal;

#[derive(Clone, Copy, Debug, PartialEq, ValueEnum)]
enum Renderer {
    Auto,
    Text,
    Curves,
}

#[derive(Parser)]
#[command(
    version,
    about = "Interactive Git commit and branch graph for Herdr",
    long_about = "Browse local Git history, branches, and diffs. Uses smooth curves in compatible Herdr panes and a Unicode graph elsewhere. No repository writes or network operations."
)]
struct Args {
    /// Git working directory (defaults to current Herdr workspace or cwd)
    #[arg(short, long)]
    repo: Option<PathBuf>,
    /// Graph output; auto negotiates Herdr graphics, text works in any terminal
    #[arg(long, value_enum, default_value_t = Renderer::Auto)]
    renderer: Renderer,
    /// Color theme; auto reads the current terminal palette
    #[arg(long, env = "HERDR_GIT_GRAPH_THEME", value_enum, default_value_t = ThemeMode::Auto)]
    theme: ThemeMode,
    /// Maximum commits to load per branch view
    #[arg(long, default_value_t = 2000, value_parser = clap::value_parser!(u32).range(1..=50000))]
    limit: u32,
    /// Seconds between background checks (slow checks automatically back off)
    #[arg(long, default_value_t = 2, value_parser = clap::value_parser!(u64).range(1..=3600))]
    refresh_interval: u64,
    /// Disable automatic refresh; use r to reload manually
    #[arg(long)]
    no_auto_refresh: bool,
    /// Open built-in sample history without a Git repository
    #[arg(long)]
    demo: bool,
    /// Open the registered plugin in a new Herdr tab
    #[arg(long, conflicts_with_all = ["demo", "snapshot", "graph_png", "check"])]
    open_pane: bool,
    /// Open a graph-only sidebar to the right of the current Herdr pane
    #[arg(long, conflicts_with_all = ["open_pane", "sidebar", "demo", "snapshot", "graph_png", "check"])]
    open_sidebar: bool,
    /// Show only the graph and commit list, suitable for narrow side panes
    #[arg(long, conflicts_with = "open_pane")]
    sidebar: bool,
    /// Write a reproducible SVG of the Ratatui screen, then exit
    #[arg(long)]
    snapshot: Option<PathBuf>,
    /// Write a PNG of the smooth graph, then exit
    #[arg(long)]
    graph_png: Option<PathBuf>,
    /// Validate and summarize the repository without opening a TUI
    #[arg(long)]
    check: bool,
    /// Width of a headless screen snapshot
    #[arg(long, default_value_t = 140, value_parser = clap::value_parser!(u16).range(20..=300))]
    width: u16,
    /// Height of a headless screen snapshot
    #[arg(long, default_value_t = 44, value_parser = clap::value_parser!(u16).range(8..=120))]
    height: u16,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let root = if args.demo {
        PathBuf::from("herdr-git-graph")
    } else {
        herdr::repository_path(args.repo.as_deref())?
    };
    if args.open_pane {
        return herdr::open_pane(&root, args.theme);
    }
    if args.open_sidebar {
        return herdr::open_sidebar(&root, args.theme);
    }
    let repo = if args.demo {
        git::demo()
    } else {
        git::load(&root, None, args.limit as usize)?
    };
    let mut app = App::new(repo, args.limit as usize, args.demo)
        .with_sidebar(args.sidebar)
        .with_refresh_interval(
            (!args.no_auto_refresh).then_some(Duration::from_secs(args.refresh_interval)),
        );
    // A headless export has no host terminal to query; retain reproducible demos.
    app.theme = Theme::new(
        if args.theme == ThemeMode::Auto && (args.snapshot.is_some() || args.graph_png.is_some()) {
            ThemeMode::Classic
        } else {
            args.theme
        },
    );
    if args.check {
        println!(
            "Repository: {}\nHEAD: {}\nBranches: {}\nCommits: {}\nGraph lanes: {}\nUncommitted files: {}",
            app.repo.root.display(),
            app.repo.head,
            app.repo.branches.len(),
            app.repo.commit_count(),
            app.graph.width,
            app.repo.worktree.files.len()
        );
        return Ok(());
    }
    if let Some(path) = &args.snapshot {
        if !args.demo
            && app.show_details
            && let Some(oid) = app.selected_oid()
        {
            app.detail = git::details(&root, oid)?;
        }
        let smooth = args.renderer == Renderer::Curves && app.theme.supports_curves();
        ui::snapshot(&mut app, path, args.width, args.height, smooth)?;
        println!("Screen snapshot: {}", path.display());
    }
    if let Some(path) = &args.graph_png {
        anyhow::ensure!(
            app.theme.supports_curves(),
            "PNG export requires --theme classic; a headless process cannot query terminal colors"
        );
        let width = (app.graph.width * 3 + 2).clamp(10, 100) as u16;
        let height = (app.graph.rows.len() * 2).clamp(2, 120) as u16;
        let view = Viewport {
            area: ratatui::layout::Rect::new(0, 0, width, height),
            top: 0,
            selected: 0,
            column_offset: 0,
        };
        herdr_git_graph::graphics::rasterize(&app.graph, view, (18, 36), app.theme)?
            .save_png(path)?;
        println!("Smooth graph: {}", path.display());
    }
    if args.snapshot.is_some() || args.graph_png.is_some() {
        return Ok(());
    }
    anyhow::ensure!(
        io::stdin().is_terminal() && io::stdout().is_terminal(),
        "대화형 터미널에서 실행하세요. 미리보기는 --demo --snapshot preview.svg를 사용하세요."
    );
    let worker = (!args.demo).then(|| Worker::start(root, args.limit as usize));
    let stop = Arc::new(AtomicBool::new(false));
    #[cfg(unix)]
    for signal in [
        signal_hook::consts::SIGTERM,
        signal_hook::consts::SIGHUP,
        signal_hook::consts::SIGINT,
    ] {
        signal_hook::flag::register(signal, stop.clone())?;
    }
    let mut session = terminal::TerminalSession::start().context("터미널 초기화 실패")?;
    let terminal = session.terminal();
    let mut probe = (args.theme == ThemeMode::Auto).then(ThemeProbe::default);
    if let Some(probe) = &probe {
        probe.query()?;
    }
    let mut surface = None;
    let mut graphics_attempted = false;
    let palette_deadline = std::time::Instant::now() + Duration::from_millis(700);
    let mut palette_fallback_reported = false;
    let mut dirty = true;
    while !app.quit && !stop.load(Ordering::Relaxed) {
        if args.renderer != Renderer::Text && !graphics_attempted && app.theme.supports_curves() {
            graphics_attempted = true;
            match Surface::connect() {
                Ok(connected) => {
                    surface = Some(connected);
                    app.renderer_status = "CURVES".into();
                    if palette_fallback_reported {
                        app.status = "Ready · terminal theme".into();
                    }
                }
                Err(error) if args.renderer == Renderer::Curves => {
                    app.status = format!("Text fallback: {error}");
                }
                Err(_) => {}
            }
            dirty = true;
        } else if args.renderer == Renderer::Curves
            && !app.theme.supports_curves()
            && !palette_fallback_reported
            && std::time::Instant::now() >= palette_deadline
        {
            app.status = "Text fallback: terminal palette unavailable; --theme classic enables fixed curve colors".into();
            palette_fallback_reported = true;
            dirty = true;
        }

        if let Some(worker) = &worker {
            while let Ok(response) = worker.receiver.try_recv() {
                dirty |= app.apply(response);
            }
            app.request_detail(worker);
            app.request_refresh(worker);
        }
        if dirty {
            let mut viewport = None;
            terminal.draw(|frame| {
                viewport = ui::draw(frame, &mut app, surface.is_some());
            })?;
            dirty = false;
            if let Some(graphics) = &mut surface {
                let result = if let Some(view) = viewport {
                    graphics.paint(&app.graph, view, app.theme)
                } else {
                    graphics.hide();
                    Ok(())
                };
                if let Err(error) = result {
                    surface = None;
                    app.renderer_status = "TEXT".into();
                    app.status = format!("Text fallback: {error}");
                    dirty = true;
                }
            }
        }
        let previous_theme = app.theme;
        let input = if let Some(probe) = &mut probe {
            probe.read(Duration::from_millis(40), &mut app.theme)
        } else {
            event::poll(Duration::from_millis(40)).and_then(|ready| {
                if ready {
                    event::read().map(Some)
                } else {
                    Ok(None)
                }
            })
        };
        let input = match input {
            Ok(input) => input,
            // A closed pane has no more input. Leave through the normal cleanup
            // path so the graphics stream and terminal guard are also dropped.
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => break,
            Err(error) => return Err(error).context("터미널 입력 실패"),
        };
        dirty |= previous_theme != app.theme;
        if let Some(input) = input {
            match input {
                Event::Key(key) if key.kind != KeyEventKind::Release => {
                    if key.code == event::KeyCode::Char('r')
                        && !app.searching
                        && !app.help
                        && let Some(probe) = &probe
                    {
                        probe.query()?;
                    }
                    app.key(key, worker.as_ref());
                    dirty = true;
                }
                Event::Mouse(mouse)
                    if matches!(
                        mouse.kind,
                        event::MouseEventKind::Down(_)
                            | event::MouseEventKind::ScrollUp
                            | event::MouseEventKind::ScrollDown
                    ) =>
                {
                    app.mouse(mouse, worker.as_ref());
                    dirty = true;
                }
                Event::Resize(_, _) => {
                    dirty = true;
                    if let Some(graphics) = &mut surface
                        && let Err(error) = graphics.refresh_size()
                    {
                        app.status = format!("Text fallback: {error}");
                        app.renderer_status = "TEXT".into();
                        surface = None;
                    }
                }
                _ => {}
            }
        }
    }
    drop(surface);
    Ok(())
}
