use anyhow::{Context, Result};
use clap::{Parser, ValueEnum};
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind},
    execute,
};
use herdr_git_graph::{
    app::{App, Worker},
    git,
    graphics::{Surface, Viewport},
    herdr, ui,
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
    /// Maximum commits to load per branch view
    #[arg(long, default_value_t = 2000, value_parser = clap::value_parser!(u32).range(1..=50000))]
    limit: u32,
    /// Open built-in sample history without a Git repository
    #[arg(long)]
    demo: bool,
    /// Open the registered plugin in a new Herdr tab
    #[arg(long, conflicts_with_all = ["demo", "snapshot", "graph_png", "check"])]
    open_pane: bool,
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

struct TerminalGuard;
impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = execute!(io::stdout(), DisableMouseCapture);
        ratatui::restore();
    }
}

fn main() -> Result<()> {
    let args = Args::parse();
    let root = if args.demo {
        PathBuf::from("herdr-git-graph")
    } else {
        herdr::repository_path(args.repo.as_deref())?
    };
    if args.open_pane {
        return herdr::open_pane(&root);
    }
    let repo = if args.demo {
        git::demo()
    } else {
        git::load(&root, None, args.limit as usize)?
    };
    let mut app = App::new(repo, args.limit as usize, args.demo);
    if args.check {
        println!(
            "Repository: {}\nHEAD: {}\nBranches: {}\nCommits: {}\nGraph lanes: {}",
            app.repo.root.display(),
            app.repo.head,
            app.repo.branches.len(),
            app.repo.commits.len(),
            app.graph.width
        );
        return Ok(());
    }
    if let Some(path) = &args.snapshot {
        if !args.demo
            && let Some(oid) = app.selected_oid()
        {
            app.detail = git::details(&root, oid)?;
        }
        ui::snapshot(
            &mut app,
            path,
            args.width,
            args.height,
            args.renderer == Renderer::Curves,
        )?;
        println!("Screen snapshot: {}", path.display());
    }
    if let Some(path) = &args.graph_png {
        let width = (app.graph.width * 3 + 2).clamp(10, 100) as u16;
        let height = (app.graph.rows.len() * 2).clamp(2, 120) as u16;
        let view = Viewport {
            area: ratatui::layout::Rect::new(0, 0, width, height),
            top: 0,
            selected: 0,
            column_offset: 0,
        };
        herdr_git_graph::graphics::rasterize(&app.graph, view, (18, 36))?.save_png(path)?;
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
    let mut terminal = ratatui::try_init().context("터미널 초기화 실패")?;
    let _guard = TerminalGuard;
    execute!(io::stdout(), EnableMouseCapture)?;
    let old_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = execute!(io::stdout(), DisableMouseCapture);
        ratatui::restore();
        old_hook(info);
    }));
    let mut surface = if args.renderer == Renderer::Text {
        None
    } else {
        match Surface::connect() {
            Ok(surface) => {
                app.renderer_status = "CURVES".into();
                Some(surface)
            }
            Err(error) => {
                if args.renderer == Renderer::Curves {
                    app.status = format!("Text fallback: {error}");
                }
                None
            }
        }
    };
    let mut dirty = true;
    while !app.quit && !stop.load(Ordering::Relaxed) {
        if let Some(worker) = &worker {
            while let Ok(response) = worker.receiver.try_recv() {
                app.apply(response);
                dirty = true;
            }
            app.request_detail(worker);
        }
        if dirty {
            let mut viewport = None;
            terminal.draw(|frame| {
                viewport = ui::draw(frame, &mut app, surface.is_some());
            })?;
            dirty = false;
            if let Some(graphics) = &mut surface {
                let result = if let Some(view) = viewport {
                    graphics.paint(&app.graph, view)
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
        if event::poll(Duration::from_millis(40))? {
            match event::read()? {
                Event::Key(key) if key.kind != KeyEventKind::Release => {
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
