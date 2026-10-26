use crate::{
    graph::Graph,
    theme::{CLASSIC_BG, Theme, ThemeMode},
};
use anyhow::{Context, Result};
use ratatui::layout::Rect;
use tiny_skia::{Color, FillRule, LineCap, Paint, PathBuilder, Pixmap, Stroke, Transform};

#[cfg(unix)]
use {
    base64::{Engine as _, engine::general_purpose::STANDARD},
    std::{
        io::{self, Write},
        os::fd::AsRawFd,
    },
};

#[derive(Clone, Copy, PartialEq)]
pub struct Viewport {
    pub area: Rect,
    pub top: usize,
    /// Terminal rows already scrolled within the top commit.
    pub top_offset: usize,
    /// Commit index and extra rows inserted after its standard two rows.
    pub expanded: Option<(usize, usize)>,
    pub selected: Option<usize>,
    pub column_offset: usize,
}

pub fn rasterize(graph: &Graph, view: Viewport, cell: (u32, u32), theme: Theme) -> Result<Pixmap> {
    let width = u32::from(view.area.width)
        .checked_mul(cell.0)
        .context("Image width overflow")?;
    let height = u32::from(view.area.height)
        .checked_mul(cell.1)
        .context("Image height overflow")?;
    anyhow::ensure!(
        u64::from(width) * u64::from(height) <= 8_000_000,
        "Graph viewport exceeds 8 megapixels"
    );
    let mut pixmap = Pixmap::new(width, height).context("Empty graph viewport")?;
    if theme.mode == ThemeMode::Classic {
        pixmap.fill(Color::from_rgba8(
            CLASSIC_BG.0,
            CLASSIC_BG.1,
            CLASSIC_BG.2,
            255,
        ));
    }
    let cw = cell.0 as f32;
    let ch = cell.1 as f32;
    let x = |column: usize| (column as f32 * 3.0 + 1.5 - view.column_offset as f32) * cw;
    let mut nodes = Vec::new();
    let mut y = -(view.top_offset as f32) * ch;
    for (index, row) in graph.rows.iter().enumerate().skip(view.top) {
        if y >= height as f32 {
            break;
        }
        let extra = view
            .expanded
            .filter(|(expanded, _)| *expanded == index)
            .map_or(0, |(_, extra)| extra);
        let row_height = (2.0 + extra as f32) * ch;
        if Some(index) == view.selected
            && let Some(bg) = theme.selection_rgb()
        {
            let rect = tiny_skia::Rect::from_xywh(0.0, y, width as f32, 2.0 * ch).unwrap();
            let mut paint = Paint::default();
            paint.set_color_rgba8(bg.0, bg.1, bg.2, 255);
            pixmap.fill_rect(rect, &paint, Transform::identity(), None);
        }
        for (column, lane) in row.above.iter().enumerate() {
            if let Some(lane) = lane {
                let mut path = PathBuilder::new();
                path.move_to(x(column), y);
                path.line_to(x(column), y + 0.5 * ch);
                stroke(&mut pixmap, path, theme.lane_rgb(lane.color), cw);
            }
        }
        for edge in &row.edges {
            let mut path = PathBuilder::new();
            path.move_to(x(edge.from), y + 0.5 * ch);
            if edge.from == edge.to {
                path.line_to(x(edge.to), y + row_height);
            } else {
                path.cubic_to(
                    x(edge.from),
                    y + 1.5 * ch,
                    x(edge.to),
                    y + ch,
                    x(edge.to),
                    y + 2.0 * ch,
                );
                // Keep the bend beside the commit, then carry its target lane
                // through any expanded file rows to the next commit.
                if extra > 0 {
                    path.line_to(x(edge.to), y + row_height);
                }
            }
            stroke(&mut pixmap, path, theme.lane_rgb(edge.color), cw);
        }
        nodes.push((
            x(row.column),
            y + 0.5 * ch,
            row.color,
            Some(index) == view.selected,
            row.uncommitted,
        ));
        y += row_height;
    }
    for (x, y, color, selected, uncommitted) in nodes {
        let rgb = theme.lane_rgb(color);
        let mut paint = Paint::default();
        let radius = (cw * 0.34).max(2.5);
        if selected {
            paint.set_color_rgba8(rgb.0, rgb.1, rgb.2, 65);
            if let Some(path) = PathBuilder::from_circle(x, y, radius * 2.1) {
                pixmap.fill_path(
                    &path,
                    &paint,
                    FillRule::Winding,
                    Transform::identity(),
                    None,
                );
            }
        }
        paint.set_color_rgba8(rgb.0, rgb.1, rgb.2, 255);
        if let Some(path) = PathBuilder::from_circle(x, y, radius) {
            pixmap.fill_path(
                &path,
                &paint,
                FillRule::Winding,
                Transform::identity(),
                None,
            );
        }
        if uncommitted {
            let bg = if selected {
                theme.selection_rgb()
            } else {
                None
            };
            if let Some(bg) = bg.or((theme.mode == ThemeMode::Classic).then_some(CLASSIC_BG)) {
                paint.set_color_rgba8(bg.0, bg.1, bg.2, 255);
            } else {
                // Erase the edge inside a hollow worktree node, revealing the pane.
                paint.blend_mode = tiny_skia::BlendMode::Clear;
            }
            if let Some(path) = PathBuilder::from_circle(x, y, radius * 0.55) {
                pixmap.fill_path(
                    &path,
                    &paint,
                    FillRule::Winding,
                    Transform::identity(),
                    None,
                );
            }
        }
    }
    Ok(pixmap)
}

fn stroke(pixmap: &mut Pixmap, path: PathBuilder, rgb: crate::theme::Rgb, cw: f32) {
    let mut paint = Paint::default();
    paint.set_color_rgba8(rgb.0, rgb.1, rgb.2, 255);
    if let Some(path) = path.finish() {
        pixmap.stroke_path(
            &path,
            &paint,
            &Stroke {
                width: (cw * 0.17).max(1.4),
                line_cap: LineCap::Round,
                ..Default::default()
            },
            Transform::identity(),
            None,
        );
    }
}

/// A conservative fallback for terminals which do not report pixels through
/// `TIOCGWINSZ`. The Kitty placement uses cell dimensions, so this only affects
/// raster quality, not the image's terminal-cell footprint.
#[cfg(unix)]
const DEFAULT_CELL_SIZE: (u32, u32) = (10, 20);
#[cfg(unix)]
const KITTY_CHUNK_SIZE: usize = 4096;

/// A lightweight Kitty graphics client. Herdr 0.9.0+ accepts these standard
/// escape sequences on the pane PTY; no graphics socket RPC is needed.
#[cfg(unix)]
pub struct Surface {
    cell: (u32, u32),
    image_id: u32,
    visible: bool,
    last_frame: Option<(u64, Viewport, Theme)>,
}

#[cfg(unix)]
impl Surface {
    pub fn connect() -> Result<Self> {
        anyhow::ensure!(
            std::env::var("HERDR_ENV").as_deref() == Ok("1"),
            "Kitty graphics are available only inside a Herdr pane"
        );
        Ok(Self {
            cell: terminal_cell_size(),
            // Process IDs are unique among concurrently running pane clients.
            // Unlike a fixed ID, this avoids deleting another application's image.
            image_id: std::process::id().max(1),
            visible: false,
            last_frame: None,
        })
    }

    pub fn refresh_size(&mut self) -> Result<()> {
        self.hide();
        self.cell = terminal_cell_size();
        self.last_frame = None;
        Ok(())
    }

    pub fn paint(&mut self, graph: &Graph, view: Viewport, theme: Theme) -> Result<()> {
        if view.area.is_empty() {
            self.hide();
            return Ok(());
        }
        if self.last_frame == Some((graph.fingerprint, view, theme)) {
            return Ok(());
        }
        let pixmap = rasterize(graph, view, self.cell, theme)?;
        let png = pixmap.encode_png()?;
        let frame = kitty_frame(
            self.image_id,
            view.area,
            pixmap.width(),
            pixmap.height(),
            &png,
        );
        let mut output = io::stdout().lock();
        output.write_all(&frame)?;
        output.flush()?;
        self.visible = true;
        self.last_frame = Some((graph.fingerprint, view, theme));
        Ok(())
    }

    pub fn hide(&mut self) {
        if self.visible {
            // `d=I` deletes both the placement and stored image data for our ID.
            // Quiet mode keeps terminal replies out of Crossterm's input stream.
            let mut output = io::stdout().lock();
            let _ = output.write_all(&kitty_delete(self.image_id));
            let _ = output.flush();
        }
        self.visible = false;
        self.last_frame = None;
    }
}

#[cfg(unix)]
impl Drop for Surface {
    fn drop(&mut self) {
        self.hide();
    }
}

#[cfg(unix)]
fn terminal_cell_size() -> (u32, u32) {
    let mut size: libc::winsize = unsafe { std::mem::zeroed() };
    let result = unsafe { libc::ioctl(io::stdout().as_raw_fd(), libc::TIOCGWINSZ, &mut size) };
    if result == 0 {
        let (columns, rows) = (u32::from(size.ws_col), u32::from(size.ws_row));
        let (pixels_x, pixels_y) = (u32::from(size.ws_xpixel), u32::from(size.ws_ypixel));
        if columns > 0 && rows > 0 && pixels_x >= columns && pixels_y >= rows {
            let cell = (pixels_x / columns, pixels_y / rows);
            if (1..=128).contains(&cell.0) && (1..=256).contains(&cell.1) {
                return cell;
            }
        }
    }
    DEFAULT_CELL_SIZE
}

#[cfg(unix)]
fn kitty_frame(image_id: u32, area: Rect, width: u32, height: u32, png: &[u8]) -> Vec<u8> {
    debug_assert!(!png.is_empty());
    let encoded = STANDARD.encode(png);
    let chunks: Vec<_> = encoded.as_bytes().chunks(KITTY_CHUNK_SIZE).collect();
    let mut frame = Vec::with_capacity(encoded.len() + chunks.len() * 32 + 32);
    // Kitty uses the cursor when it receives the last image-data chunk. Save and
    // restore it so the overlay does not perturb Ratatui's cursor bookkeeping.
    frame.extend_from_slice(b"\x1b7");
    frame.extend_from_slice(format!("\x1b[{};{}H", area.y + 1, area.x + 1).as_bytes());
    for (index, chunk) in chunks.iter().enumerate() {
        let more = u8::from(index + 1 < chunks.len());
        if index == 0 {
            frame.extend_from_slice(
                format!(
                    "\x1b_Ga=T,f=100,s={width},v={height},i={image_id},p=1,c={},r={},z=-1,C=1,q=2,m={more};",
                    area.width, area.height,
                )
                .as_bytes(),
            );
        } else {
            // All transmission and placement metadata belongs to chunk one;
            // Kitty continuations need only say whether another chunk follows.
            frame.extend_from_slice(format!("\x1b_Gm={more};").as_bytes());
        }
        frame.extend_from_slice(chunk);
        frame.extend_from_slice(b"\x1b\\");
    }
    frame.extend_from_slice(b"\x1b8");
    frame
}

#[cfg(unix)]
fn kitty_delete(image_id: u32) -> Vec<u8> {
    format!("\x1b_Ga=d,d=I,i={image_id},q=2;\x1b\\").into_bytes()
}

#[cfg(not(unix))]
pub struct Surface;
#[cfg(not(unix))]
impl Surface {
    pub fn connect() -> Result<Self> {
        anyhow::bail!("Herdr graphics currently supports macOS/Linux")
    }
    pub fn refresh_size(&mut self) -> Result<()> {
        Ok(())
    }
    pub fn paint(&mut self, _: &Graph, _: Viewport, _: Theme) -> Result<()> {
        Ok(())
    }
    pub fn hide(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inherited_graph_has_transparent_background_and_the_same_selection_as_text() {
        let mut commits = crate::git::demo().commits;
        commits[0].oid = crate::git::WORKTREE_OID.into();
        let graph = Graph::build(&commits);
        for (bg, fg) in [
            ((250, 250, 250), (30, 30, 30)),
            ((20, 20, 20), (230, 230, 230)),
        ] {
            let theme = Theme {
                background: Some(bg),
                foreground: Some(fg),
                palette: [Some((51, 170, 136)); 6],
                ..Theme::default()
            };
            let view = Viewport {
                area: Rect::new(0, 0, 14, 16),
                top: 0,
                top_offset: 0,
                expanded: None,
                selected: Some(1),
                column_offset: 0,
            };
            let image = rasterize(&graph, view, (10, 20), theme).unwrap();
            assert_eq!(image.pixel(0, 0).unwrap().alpha(), 0);
            // Hollow worktree nodes clear their center instead of painting a dark dot.
            assert_eq!(
                image
                    .pixel((graph.rows[0].column * 30 + 15) as u32, 10)
                    .unwrap()
                    .alpha(),
                0
            );
            let selected = theme.selection_rgb().unwrap();
            for y in [40, 60] {
                let pixel = image.pixel(0, y).unwrap();
                assert_eq!(
                    (pixel.red(), pixel.green(), pixel.blue(), pixel.alpha()),
                    (selected.0, selected.1, selected.2, 255)
                );
            }
            assert_eq!(image.pixel(0, 80).unwrap().alpha(), 0);
            assert!(
                image
                    .pixels()
                    .iter()
                    .any(|p| (p.red(), p.green(), p.blue(), p.alpha()) == (51, 170, 136, 255))
            );
        }
    }

    #[test]
    fn no_selection_leaves_background_and_node_ring_unpainted() {
        let mut commits = crate::git::demo().commits;
        commits.truncate(1);
        commits[0].parents.clear();
        let graph = Graph::build(&commits);
        let view = Viewport {
            area: Rect::new(0, 0, 4, 2),
            top: 0,
            top_offset: 0,
            expanded: None,
            selected: None,
            column_offset: 0,
        };
        for theme in [
            Theme::new(ThemeMode::Classic),
            Theme {
                background: Some((20, 20, 20)),
                foreground: Some((230, 230, 230)),
                ..Theme::default()
            },
            Theme::default(),
        ] {
            let image = rasterize(&graph, view, (10, 20), theme).unwrap();
            let selected = rasterize(
                &graph,
                Viewport {
                    selected: Some(0),
                    ..view
                },
                (10, 20),
                theme,
            )
            .unwrap();
            let background = if theme.mode == ThemeMode::Classic {
                (CLASSIC_BG.0, CLASSIC_BG.1, CLASSIC_BG.2, 255)
            } else {
                (0, 0, 0, 0)
            };
            for x in [0, 21] {
                let pixel = image.pixel(x, 10).unwrap();
                assert_eq!(
                    (pixel.red(), pixel.green(), pixel.blue(), pixel.alpha()),
                    background,
                    "An unselected graph must not paint a row background or node ring"
                );
            }
            assert!(image.pixel(15, 10).unwrap().alpha() > 0);
            assert_ne!(image.pixel(21, 10), selected.pixel(21, 10));
            if theme.selection_rgb().is_some() {
                assert_ne!(image.pixel(0, 10), selected.pixel(0, 10));
            }
        }
    }

    fn expansion_graph() -> Graph {
        let mut commits = crate::git::demo().commits;
        commits.truncate(3);
        commits[0].parents = vec![commits[1].oid.clone(), commits[2].oid.clone()];
        commits[1].parents = vec![commits[2].oid.clone()];
        commits[2].parents.clear();
        Graph::build(&commits)
    }

    #[test]
    fn expanded_rows_shift_nodes_and_preserve_lanes_without_extending_selection() {
        let graph = expansion_graph();
        let theme = Theme::new(ThemeMode::Classic);
        let view = Viewport {
            area: Rect::new(0, 0, 7, 9),
            top: 0,
            top_offset: 0,
            expanded: Some((0, 3)),
            selected: Some(0),
            column_offset: 0,
        };
        let image = rasterize(&graph, view, (10, 20), theme).unwrap();
        let original = rasterize(
            &graph,
            Viewport {
                expanded: None,
                ..view
            },
            (10, 20),
            theme,
        )
        .unwrap();
        let background = original.pixel(0, 40).unwrap();
        assert_ne!(image.pixel(0, 20).unwrap(), background);
        for y in 40..100 {
            assert_eq!(image.pixel(0, y).unwrap(), background);
            // Both the straight first-parent lane and curved second-parent
            // lane must continue through every inserted terminal row.
            for x in [15, 45] {
                assert_ne!(image.pixel(x, y).unwrap(), background);
            }
        }
        assert_eq!(image.pixel(17, 50).unwrap(), background);
        for (x, old_y) in [(17, 50), (47, 90)] {
            assert_ne!(original.pixel(x, old_y).unwrap(), background);
            assert_eq!(
                image.pixel(x, old_y + 60),
                original.pixel(x, old_y),
                "Later commit nodes must move down by the expanded row count"
            );
        }
        let stride = image.width() as usize * 4;
        assert_eq!(
            &image.data()[100 * stride..160 * stride],
            &original.data()[40 * stride..100 * stride],
            "All later graph rows, not only the first following node, must shift"
        );
    }

    #[test]
    fn scrolled_expansion_clips_to_top_offset_and_renders_partial_bottom_commit() {
        let graph = expansion_graph();
        let theme = Theme::new(ThemeMode::Classic);
        let view = Viewport {
            area: Rect::new(0, 0, 7, 9),
            top: 0,
            top_offset: 0,
            expanded: Some((0, 3)),
            selected: Some(0),
            column_offset: 0,
        };
        let full = rasterize(&graph, view, (10, 20), theme).unwrap();
        for top_offset in [1, 3, 4] {
            let clipped = rasterize(
                &graph,
                Viewport {
                    area: Rect::new(0, 0, 7, 5),
                    top_offset,
                    ..view
                },
                (10, 20),
                theme,
            )
            .unwrap();
            let start = top_offset * 20 * full.width() as usize * 4;
            // Tiny-skia can tessellate a cubic differently when its bounds
            // cross the clip edge. Allow small antialiasing differences rather
            // than demanding byte-identical pixels from independently clipped
            // curves; shifted nodes, missing lanes, or selection still fail.
            for (index, (actual, expected)) in clipped
                .data()
                .iter()
                .zip(&full.data()[start..start + clipped.data().len()])
                .enumerate()
            {
                assert!(
                    actual.abs_diff(*expected) <= 32,
                    "Scrolling {top_offset} terminal rows differs at pixel ({}, {}), channel {}: {actual} vs {expected}",
                    index / 4 % full.width() as usize,
                    index / 4 / full.width() as usize,
                    index % 4,
                );
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn kitty_frame_transmits_png_in_bounded_chunks_and_places_it_without_moving_cursor() {
        let png = vec![0xA5; 4096];
        let frame = kitty_frame(42, Rect::new(2, 3, 7, 9), 70, 180, &png);
        assert!(frame.starts_with(
            b"\x1b7\x1b[4;3H\x1b_Ga=T,f=100,s=70,v=180,i=42,p=1,c=7,r=9,z=-1,C=1,q=2,m=1;"
        ));
        assert!(frame.ends_with(b"\x1b8"));

        let start = frame
            .windows(3)
            .position(|part| part == b"\x1b_G")
            .expect("first Kitty graphics sequence");
        let contents = &frame[start..frame.len() - 2];
        let messages: Vec<_> = contents
            .split(|byte| *byte == b'\\')
            .filter(|message| !message.is_empty())
            .collect();
        assert_eq!(messages.len(), 2);
        let mut encoded = Vec::new();
        for (index, message) in messages.iter().enumerate() {
            assert!(message.ends_with(&[0x1b]));
            let message = &message[..message.len() - 1];
            let data = message.splitn(2, |byte| *byte == b';').nth(1).unwrap();
            assert!(data.len() <= KITTY_CHUNK_SIZE);
            encoded.extend_from_slice(data);
            if index > 0 {
                assert!(message.starts_with(b"\x1b_Gm=0;"));
                assert!(!message.windows(2).any(|part| part == b"a="));
            }
        }
        assert_eq!(STANDARD.decode(encoded).unwrap(), png);
    }

    #[cfg(unix)]
    #[test]
    fn kitty_delete_releases_only_this_clients_image() {
        assert_eq!(kitty_delete(42), b"\x1b_Ga=d,d=I,i=42,q=2;\x1b\\");
    }

    #[test]
    fn renders_visible_slice_with_antialiasing_and_bounds() {
        let graph = Graph::build(&crate::git::demo().commits);
        let view = Viewport {
            area: Rect::new(0, 0, 14, 16),
            top: 1,
            top_offset: 0,
            expanded: None,
            selected: Some(2),
            column_offset: 0,
        };
        let image = rasterize(&graph, view, (10, 20), Theme::new(ThemeMode::Classic)).unwrap();
        assert_eq!((image.width(), image.height()), (140, 320));
        let colors: std::collections::HashSet<_> = image.data().as_chunks::<4>().0.iter().collect();
        assert!(
            colors.len() > 30,
            "Curves should have antialiased intermediate colors"
        );
        assert!(rasterize(&graph, view, (4000, 4000), Theme::new(ThemeMode::Classic)).is_err());
    }
}
