//! Render light-theme documentation previews without changing terminal settings.
use herdr_git_graph::{
    app::App,
    git,
    theme::{Theme, ThemeMode},
    ui,
};
use std::path::Path;

fn main() -> anyhow::Result<()> {
    // Ghostty's Tomorrow palette, as a light terminal would return to --theme auto.
    let theme = Theme {
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
    };
    let docs = Path::new(env!("CARGO_MANIFEST_DIR")).join("docs");
    for (filename, sidebar, width, height) in [
        ("preview-light.svg", false, 140, 56),
        ("preview-sidebar-light.svg", true, 40, 26),
    ] {
        let mut app = App::new(git::demo(), 2000, true).with_sidebar(sidebar);
        app.theme = theme;
        let path = docs.join(filename);
        ui::snapshot(&mut app, &path, width, height, true)?;
        println!("Screen snapshot: {}", path.display());
    }
    Ok(())
}
