use crate::{
    app::{App, Focus},
    graph::PALETTE,
    graphics::Viewport,
};
use ratatui::{
    Frame, Terminal,
    backend::TestBackend,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Clear, Paragraph},
};
use std::{fmt::Write, path::Path};

pub const BG_RGB: (u8, u8, u8) = (12, 17, 24);
pub const SELECT_RGB: (u8, u8, u8) = (28, 46, 59);
const BG: Color = Color::Rgb(12, 17, 24);
const PANEL: Color = Color::Rgb(15, 22, 31);
const FG: Color = Color::Rgb(219, 229, 239);
const MUTED: Color = Color::Rgb(117, 137, 158);
const BORDER: Color = Color::Rgb(40, 55, 70);
const ACCENT: Color = Color::Rgb(94, 234, 212);
const SELECT: Color = Color::Rgb(28, 46, 59);

pub fn clean(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c == '\t' {
                ' '
            } else if c.is_control() {
                '�'
            } else {
                c
            }
        })
        .collect()
}

fn color(index: usize) -> Color {
    let (r, g, b) = PALETTE[index % PALETTE.len()];
    Color::Rgb(r, g, b)
}

fn block(title: &str, focused: bool) -> Block<'_> {
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(if focused { ACCENT } else { BORDER }))
        .title(Span::styled(
            format!(" {title} "),
            Style::default().fg(if focused { ACCENT } else { MUTED }),
        ))
}

pub fn draw(frame: &mut Frame, app: &mut App, smooth: bool) -> Option<Viewport> {
    let area = frame.area();
    frame.render_widget(Block::default().style(Style::default().bg(BG).fg(FG)), area);
    app.history_area = Rect::default();
    app.branches_area = Rect::default();
    app.detail_area = Rect::default();
    if area.width < 64 || area.height < 18 {
        frame.render_widget(
            Paragraph::new(
                "Herdr Git Graph\n\nEnlarge this pane to at least 64 × 18.\nPress q to close.",
            )
            .style(Style::default().fg(ACCENT)),
            area,
        );
        return None;
    }
    let rows = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(2),
        Constraint::Min(8),
        Constraint::Length(2),
    ])
    .split(area);
    header(frame, app, rows[0]);
    let query = if app.searching {
        format!(" / {}▏", clean(&app.query))
    } else if !app.query.is_empty() {
        format!(" / {}  ·  n / N to jump", clean(&app.query))
    } else {
        " / Search commits, authors, refs or hashes".into()
    };
    let toolbar = Layout::horizontal([Constraint::Min(20), Constraint::Length(28)]).split(rows[1]);
    frame.render_widget(
        Paragraph::new(query).style(Style::default().fg(if app.searching {
            ACCENT
        } else {
            MUTED
        })),
        toolbar[0],
    );
    frame.render_widget(
        Paragraph::new(format!(
            "{}  ·  {} commits ",
            if app.demo {
                "DEMO"
            } else {
                &app.renderer_status
            },
            app.repo.commits.len()
        ))
        .right_aligned()
        .style(Style::default().fg(MUTED)),
        toolbar[1],
    );

    let columns = Layout::horizontal([
        Constraint::Length(if area.width >= 96 || app.focus == Focus::Branches {
            25
        } else {
            0
        }),
        Constraint::Min(30),
    ])
    .spacing(1)
    .split(rows[2]);
    if columns[0].width > 0 {
        branches(frame, app, columns[0]);
    }
    let main_rows = if app.show_details {
        Layout::vertical([Constraint::Percentage(62), Constraint::Percentage(38)])
            .spacing(1)
            .split(columns[1])
    } else {
        Layout::vertical([Constraint::Percentage(100), Constraint::Length(0)]).split(columns[1])
    };
    let view = history(frame, app, main_rows[0], smooth);
    if app.show_details {
        details(frame, app, main_rows[1]);
    }
    footer(frame, app, rows[3]);
    if app.help {
        let width = 61.min(area.width - 2);
        let height = 19.min(area.height - 2);
        let modal = Rect::new(
            (area.width - width) / 2,
            (area.height - height) / 2,
            width,
            height,
        );
        frame.render_widget(Clear, modal);
        let help = "\n  Tab / Shift-Tab    Move between panels\n  ↑ ↓  /  j k         Select commit or branch\n  Enter               Open details / apply branch filter\n  PgUp / PgDn         Move one page\n  Home / End  ·  g G   First / last item\n  h / l               Pan graph / scroll diff horizontally\n  /                   Search loaded history\n  n / N               Next / previous search match\n  a                   Show all refs\n  r                   Reload local repository\n  d                   Toggle details\n  Mouse               Click to select · wheel to scroll\n  q / Ctrl-C          Quit\n\n  Search highlights history without removing graph nodes.\n  Esc or ? closes this help.";
        frame.render_widget(
            Paragraph::new(help)
                .style(Style::default().fg(FG).bg(PANEL))
                .block(block("Keyboard shortcuts", true)),
            modal,
        );
        return None;
    }
    view
}

fn header(frame: &mut Frame, app: &App, area: Rect) {
    let name = app
        .repo
        .root
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| app.repo.root.display().to_string());
    let lines = vec![
        Line::from(vec![
            Span::styled("  ◈  ", Style::default().fg(ACCENT)),
            Span::styled("HERDR ", Style::default().fg(MUTED)),
            Span::styled(
                "GIT GRAPH",
                Style::default().fg(FG).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::styled(format!("     {}", clean(&name)), Style::default().fg(FG)),
            Span::styled("  /  ", Style::default().fg(BORDER)),
            Span::styled(clean(&app.repo.head), Style::default().fg(ACCENT)),
        ]),
    ];
    frame.render_widget(
        Paragraph::new(lines).style(Style::default().bg(PANEL)),
        area,
    );
}

fn branches(frame: &mut Frame, app: &mut App, area: Rect) {
    let b = block("BRANCHES", app.focus == Focus::Branches);
    let inner = b.inner(area);
    frame.render_widget(b, area);
    let available = usize::from(inner.height.saturating_sub(3));
    app.branches_area = Rect {
        height: (available + 1) as u16,
        ..inner
    };
    if app.branch_selected > 0 {
        let i = app.branch_selected - 1;
        if i < app.branch_top {
            app.branch_top = i;
        }
        if i >= app.branch_top + available.max(1) {
            app.branch_top = i + 1 - available.max(1);
        }
    }
    let row = |name: String, selected: bool, active: bool| {
        Line::from(vec![
            Span::styled(
                if active { " ● " } else { "   " },
                Style::default().fg(ACCENT),
            ),
            Span::styled(name, Style::default().fg(if selected { FG } else { MUTED })),
        ])
        .style(Style::default().bg(if selected { SELECT } else { BG }))
    };
    let mut lines = vec![row(
        "All branches".into(),
        app.branch_selected == 0,
        app.reference.is_none(),
    )];
    for (i, branch) in app
        .repo
        .branches
        .iter()
        .enumerate()
        .skip(app.branch_top)
        .take(available)
    {
        lines.push(row(
            clean(&branch.name),
            app.branch_selected == i + 1,
            app.reference.as_ref() == Some(&branch.reference),
        ));
    }
    frame.render_widget(Paragraph::new(lines), inner);
    if inner.height > 5 {
        let foot = Rect::new(inner.x, inner.bottom() - 2, inner.width, 2);
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(format!(
                    " {} refs · Enter to apply",
                    app.repo.branches.len()
                )),
                Line::from(" Local history · r to reload"),
            ])
            .style(Style::default().fg(MUTED)),
            foot,
        );
    }
}

fn history(frame: &mut Frame, app: &mut App, area: Rect, smooth: bool) -> Option<Viewport> {
    let b = block(
        if app.loading {
            "HISTORY · loading…"
        } else {
            "HISTORY"
        },
        app.focus == Focus::History,
    );
    let inner = b.inner(area);
    frame.render_widget(b, area);
    if inner.height < 2 || inner.width < 10 {
        return None;
    }
    let body = Rect::new(inner.x, inner.y + 1, inner.width, inner.height - 1);
    app.history_area = body;
    let count = usize::from(body.height).div_ceil(2).max(1);
    if app.selected < app.top {
        app.top = app.selected;
    }
    if app.selected >= app.top + count {
        app.top = app.selected + 1 - count;
    }
    let graph_width = ((app.graph.width * 3 + 2).max(10) as u16)
        .min(36)
        .min(inner.width / 3);
    let cols = Layout::horizontal([
        Constraint::Length(graph_width),
        Constraint::Min(10),
        Constraint::Length(if inner.width >= 72 { 19 } else { 0 }),
    ])
    .spacing(1)
    .split(body);
    let col_header = Rect::new(inner.x, inner.y, inner.width, 1);
    frame.render_widget(
        Paragraph::new(" GRAPH").style(Style::default().fg(MUTED).bg(PANEL)),
        col_header,
    );
    frame.render_widget(
        Paragraph::new("COMMIT / DESCRIPTION").style(Style::default().fg(MUTED).bg(PANEL)),
        Rect::new(cols[1].x, inner.y, cols[1].width, 1),
    );
    if cols[2].width > 0 {
        frame.render_widget(
            Paragraph::new("AUTHOR · DATE").style(Style::default().fg(MUTED).bg(PANEL)),
            Rect::new(cols[2].x, inner.y, cols[2].width, 1),
        );
    }
    if app.repo.commits.is_empty() {
        frame.render_widget(
            Paragraph::new("\n No commits yet.\n Create a commit, then press r.")
                .style(Style::default().fg(MUTED)),
            body,
        );
        return None;
    }
    for (i, commit) in app
        .repo
        .commits
        .iter()
        .enumerate()
        .skip(app.top)
        .take(count)
    {
        let y = body.y + ((i - app.top) * 2) as u16;
        if y >= body.bottom() {
            break;
        }
        let selected = app.selected == i;
        let bg = if selected { SELECT } else { BG };
        frame.render_widget(
            Block::default().style(Style::default().bg(bg)),
            Rect::new(body.x, y, body.width, 1),
        );
        let short: String = commit.oid.chars().take(7).collect();
        let is_match = !app.query.is_empty()
            && [&commit.subject, &commit.author, &commit.refs, &commit.oid]
                .iter()
                .any(|s| s.to_lowercase().contains(&app.query.to_lowercase()));
        let line = Line::from(vec![
            Span::styled(
                format!("{short}  "),
                Style::default().fg(color(app.graph.rows[i].color)),
            ),
            Span::styled(
                clean(&commit.subject),
                Style::default()
                    .fg(if is_match {
                        Color::Rgb(251, 191, 106)
                    } else {
                        FG
                    })
                    .add_modifier(if selected {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    }),
            ),
        ]);
        frame.render_widget(
            Paragraph::new(line),
            Rect::new(cols[1].x, y, cols[1].width, 1),
        );
        if y + 1 < body.bottom() {
            frame.render_widget(
                Paragraph::new(Span::styled(
                    clean(&commit.refs),
                    Style::default().fg(ACCENT),
                )),
                Rect::new(cols[1].x, y + 1, cols[1].width, 1),
            );
        }
        if cols[2].width > 0 {
            frame.render_widget(
                Paragraph::new(clean(&commit.author)).style(Style::default().fg(if selected {
                    FG
                } else {
                    MUTED
                })),
                Rect::new(cols[2].x, y, cols[2].width, 1),
            );
            if y + 1 < body.bottom() {
                frame.render_widget(
                    Paragraph::new(commit.date.clone()).style(Style::default().fg(MUTED)),
                    Rect::new(cols[2].x, y + 1, cols[2].width, 1),
                );
            }
        }
        if !smooth {
            text_graph(frame, &app.graph.rows[i], cols[0], y, app.graph_offset);
        }
    }
    Some(Viewport {
        area: cols[0],
        top: app.top,
        selected: app.selected,
        column_offset: app.graph_offset,
    })
}

fn text_graph(frame: &mut Frame, row: &crate::graph::Row, area: Rect, y: u16, offset: usize) {
    let mut put = |column: usize, line: u16, symbol: &str, c: usize, crossing: bool| {
        let x = i32::from(area.x) + column as i32 - offset as i32;
        if x >= i32::from(area.x)
            && x < i32::from(area.right())
            && line < area.bottom()
            && let Some(cell) = frame.buffer_mut().cell_mut((x as u16, line))
        {
            let symbol = if crossing
                && matches!(cell.symbol(), "│" | "─" | "╭" | "╮" | "╰" | "╯")
                && cell.symbol() != symbol
            {
                "┼"
            } else {
                symbol
            };
            cell.set_symbol(symbol).set_fg(color(c));
        }
    };
    for (i, lane) in row.above.iter().enumerate() {
        if let Some(lane) = lane {
            put(i * 3 + 1, y, "│", lane.color, false);
        }
    }
    // Draw pass-throughs first so intersections remain explicit in text mode.
    for edge in row.edges.iter().filter(|e| e.from == e.to) {
        put(edge.from * 3 + 1, y + 1, "│", edge.color, false);
    }
    for edge in row.edges.iter().filter(|e| e.from != e.to) {
        let a = edge.from * 3 + 1;
        let b = edge.to * 3 + 1;
        for x in a.min(b) + 1..a.max(b) {
            put(x, y + 1, "─", edge.color, true);
        }
        put(a, y + 1, if a < b { "╰" } else { "╯" }, edge.color, true);
        put(b, y + 1, if a < b { "╮" } else { "╭" }, edge.color, true);
    }
    put(row.column * 3 + 1, y, "●", row.color, false);
}

fn details(frame: &mut Frame, app: &mut App, area: Rect) {
    let b = block("COMMIT INSPECTOR", app.focus == Focus::Details);
    let inner = b.inner(area);
    app.detail_area = area;
    frame.render_widget(b, area);
    let lines: Vec<_> = app
        .detail
        .lines()
        .skip(app.detail_scroll)
        .take(usize::from(inner.height))
        .map(|line| {
            let fg = if line.starts_with('+') && !line.starts_with("+++") {
                Color::Rgb(163, 230, 153)
            } else if line.starts_with('-') && !line.starts_with("---") {
                Color::Rgb(251, 113, 133)
            } else if line.starts_with("@@") {
                Color::Rgb(167, 139, 250)
            } else if line.starts_with("commit ") || line.starts_with("diff ") {
                ACCENT
            } else {
                MUTED
            };
            Line::styled(clean(line), Style::default().fg(fg))
        })
        .collect();
    frame.render_widget(
        Paragraph::new(lines).scroll((0, app.detail_horizontal)),
        inner,
    );
}

fn footer(frame: &mut Frame, app: &App, area: Rect) {
    let range = if app.repo.commits.is_empty() {
        "0 / 0".into()
    } else {
        format!(
            "{} / {}{}",
            app.selected + 1,
            app.repo.commits.len(),
            if app.repo.commits.len() == app.limit {
                " · limit reached"
            } else {
                ""
            }
        )
    };
    let rows = Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).split(area);
    let cols = Layout::horizontal([Constraint::Min(10), Constraint::Length(32)]).split(rows[0]);
    frame.render_widget(
        Paragraph::new(format!(" {}", clean(&app.status))).style(Style::default().fg(MUTED)),
        cols[0],
    );
    frame.render_widget(
        Paragraph::new(format!("{range} "))
            .right_aligned()
            .style(Style::default().fg(MUTED)),
        cols[1],
    );
    let keys = if app.searching {
        " Enter apply   Esc clear"
    } else {
        " Tab panels   ↑↓ navigate   / search   a all   r reload   d details   ? help   q quit"
    };
    frame.render_widget(
        Paragraph::new(keys).style(Style::default().bg(PANEL).fg(ACCENT)),
        rows[1],
    );
}

pub fn snapshot(
    app: &mut App,
    path: &Path,
    width: u16,
    height: u16,
    smooth: bool,
) -> anyhow::Result<()> {
    use base64::Engine;
    let mut terminal = Terminal::new(TestBackend::new(width, height))?;
    let mut viewport = None;
    terminal.draw(|frame| {
        viewport = draw(frame, app, smooth);
    })?;
    let buffer = terminal.backend().buffer();
    let mut svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{}\" height=\"{}\" viewBox=\"0 0 {} {}\"><rect width=\"100%\" height=\"100%\" fill=\"#0c1118\"/><g font-family=\"Menlo, 'DejaVu Sans Mono', monospace\" font-size=\"14\">",
        u32::from(width) * 9,
        u32::from(height) * 20,
        u32::from(width) * 9,
        u32::from(height) * 20
    );
    for y in 0..height {
        for x in 0..width {
            let cell = &buffer[(x, y)];
            if let Color::Rgb(r, g, b) = cell.bg {
                write!(
                    svg,
                    "<rect x=\"{}\" y=\"{}\" width=\"9\" height=\"20\" fill=\"#{r:02x}{g:02x}{b:02x}\"/>",
                    u32::from(x) * 9,
                    u32::from(y) * 20
                )?;
            }
        }
        for x in 0..width {
            let cell = &buffer[(x, y)];
            if cell.symbol().trim().is_empty() {
                continue;
            }
            let (r, g, b) = if let Color::Rgb(r, g, b) = cell.fg {
                (r, g, b)
            } else {
                (219, 229, 239)
            };
            write!(
                svg,
                "<text x=\"{}\" y=\"{}\" fill=\"#{r:02x}{g:02x}{b:02x}\"{}>{}</text>",
                u32::from(x) * 9,
                u32::from(y) * 20 + 15,
                if cell.modifier.contains(Modifier::BOLD) {
                    " font-weight=\"bold\""
                } else {
                    ""
                },
                xml(cell.symbol())
            )?;
        }
    }
    svg.push_str("</g>");
    if smooth && let Some(view) = viewport {
        let png = crate::graphics::rasterize(&app.graph, view, (9, 20))?.encode_png()?;
        let encoded = base64::engine::general_purpose::STANDARD.encode(png);
        write!(
            svg,
            "<image x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" href=\"data:image/png;base64,{encoded}\"/>",
            u32::from(view.area.x) * 9,
            u32::from(view.area.y) * 20,
            u32::from(view.area.width) * 9,
            u32::from(view.area.height) * 20
        )?;
    }
    svg.push_str("</svg>");
    std::fs::write(path, svg)?;
    Ok(())
}

fn xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn responsive_views_and_wide_text_do_not_panic() {
        for (width, height) in [(20, 8), (64, 18), (95, 30), (132, 42), (200, 60)] {
            let mut app = App::new(crate::git::demo(), 2000, true);
            app.selected = 8;
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal
                .draw(|frame| {
                    draw(frame, &mut app, false);
                })
                .unwrap();
            app.help = true;
            terminal
                .draw(|frame| {
                    draw(frame, &mut app, true);
                })
                .unwrap();
        }
    }
    #[test]
    fn terminal_control_bytes_cannot_escape_into_ui() {
        assert_eq!(clean("test\u{1b}[31m\tmsg"), "test�[31m msg");
    }
}
