use crate::{
    graph::Graph,
    theme::{CLASSIC_BG, Theme, ThemeMode},
};
use anyhow::{Context, Result};
use ratatui::layout::Rect;
use tiny_skia::{Color, FillRule, LineCap, Paint, PathBuilder, Pixmap, Stroke, Transform};

#[derive(Clone, Copy, PartialEq)]
pub struct Viewport {
    pub area: Rect,
    pub top: usize,
    pub selected: usize,
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
    let visible = usize::from(view.area.height).div_ceil(2);
    let mut nodes = Vec::new();
    for (index, row) in graph.rows.iter().enumerate().skip(view.top).take(visible) {
        let y = ((index - view.top) * 2) as f32 * ch;
        if index == view.selected
            && let Some(bg) = theme.selection_rgb()
        {
            let rect = tiny_skia::Rect::from_xywh(0.0, y, width as f32, ch).unwrap();
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
                path.line_to(x(edge.to), y + 2.0 * ch);
            } else {
                path.cubic_to(
                    x(edge.from),
                    y + 1.5 * ch,
                    x(edge.to),
                    y + ch,
                    x(edge.to),
                    y + 2.0 * ch,
                );
            }
            stroke(&mut pixmap, path, theme.lane_rgb(edge.color), cw);
        }
        nodes.push((
            x(row.column),
            y + 0.5 * ch,
            row.color,
            index == view.selected,
            row.uncommitted,
        ));
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

#[cfg(unix)]
pub struct Surface {
    endpoint: crate::herdr::socket::Endpoint,
    stream: Option<std::os::unix::net::UnixStream>,
    cell: (u32, u32),
    last_frame: Option<(u64, Viewport, Theme)>,
}

#[cfg(unix)]
impl Surface {
    pub fn connect() -> Result<Self> {
        let endpoint = crate::herdr::socket::Endpoint::from_env()?;
        let mut surface = Self {
            endpoint,
            stream: None,
            cell: (0, 0),
            last_frame: None,
        };
        surface.refresh_size()?;
        Ok(surface)
    }

    pub fn refresh_size(&mut self) -> Result<()> {
        let info = self.endpoint.request(
            "pane.graphics.info",
            serde_json::json!({"pane_id":self.endpoint.pane}),
        )?;
        let width = info["cell_width_px"].as_u64().unwrap_or(0);
        let height = info["cell_height_px"].as_u64().unwrap_or(0);
        anyhow::ensure!(
            (1..=128).contains(&width) && (1..=256).contains(&height),
            "터미널 픽셀 크기를 확인할 수 없습니다."
        );
        self.cell = (width as u32, height as u32);
        self.last_frame = None;
        Ok(())
    }

    pub fn paint(&mut self, graph: &Graph, view: Viewport, theme: Theme) -> Result<()> {
        use std::io::Write;
        if view.area.is_empty() {
            self.hide();
            return Ok(());
        }
        if self.last_frame == Some((graph.fingerprint, view, theme)) {
            return Ok(());
        }
        let pixmap = rasterize(graph, view, self.cell, theme)?;
        if self.stream.is_none() {
            let mut stream = self.endpoint.connect()?;
            self.endpoint.request_on(&mut stream, "pane.graphics.stream", serde_json::json!({"pane_id":self.endpoint.pane,"layer_id":"git-graph","z_index":1}))?;
            self.stream = Some(stream);
        }
        let stream = self.stream.as_mut().unwrap();
        let data = pixmap.encode_png()?;
        let area = view.area;
        let header = serde_json::json!({"format":"png", "image_width":pixmap.width(), "image_height":pixmap.height(), "data_length":data.len(), "placement":{"viewport_col":area.x,"viewport_row":area.y,"grid_cols":area.width,"grid_rows":area.height}});
        // Header and bytes are one transaction. A closed stream removes its layer.
        let mut frame = serde_json::to_vec(&header)?;
        frame.push(b'\n');
        frame.extend(data);
        stream.write_all(&frame)?;
        self.last_frame = Some((graph.fingerprint, view, theme));
        Ok(())
    }

    pub fn hide(&mut self) {
        self.stream.take();
        self.last_frame = None;
    }
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
                selected: 1,
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
            let pixel = image.pixel(0, 40).unwrap();
            let selected = theme.selection_rgb().unwrap();
            assert_eq!(
                (pixel.red(), pixel.green(), pixel.blue(), pixel.alpha()),
                (selected.0, selected.1, selected.2, 255)
            );
            assert!(
                image
                    .pixels()
                    .iter()
                    .any(|p| (p.red(), p.green(), p.blue(), p.alpha()) == (51, 170, 136, 255))
            );
        }
    }

    #[test]
    fn renders_visible_slice_with_antialiasing_and_bounds() {
        let graph = Graph::build(&crate::git::demo().commits);
        let view = Viewport {
            area: Rect::new(0, 0, 14, 16),
            top: 1,
            selected: 2,
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
