use crate::{
    app::{App, Focus},
    graphics::Viewport,
    theme::Theme,
};
use ratatui::{
    Frame, Terminal,
    backend::TestBackend,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Clear, Paragraph, Wrap},
};
use std::{fmt::Write, path::Path};

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

fn block(theme: Theme, title: &str, focused: bool) -> Block<'_> {
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(if focused {
            theme.accent()
        } else {
            theme.border()
        }))
        .title(Span::styled(
            format!(" {title} "),
            Style::default().fg(if focused {
                theme.accent()
            } else {
                theme.muted().fg.unwrap_or(Color::Reset)
            }),
        ))
}

pub fn draw(frame: &mut Frame, app: &mut App, smooth: bool) -> Option<Viewport> {
    let theme = app.theme;
    let area = frame.area();
    frame.render_widget(
        Block::default().style(Style::default().bg(theme.bg()).fg(theme.fg())),
        area,
    );
    app.history_area = Rect::default();
    app.branches_area = Rect::default();
    app.detail_area = Rect::default();
    let (min_width, min_height) = if app.sidebar { (24, 8) } else { (64, 18) };
    if area.width < min_width || area.height < min_height {
        frame.render_widget(
            Paragraph::new(format!(
                "Herdr Git Graph\n\nEnlarge to {min_width} × {min_height}.\nPress q to close."
            ))
            .style(Style::default().fg(theme.accent())),
            area,
        );
        return None;
    }
    if app.help {
        help(frame, app, area);
        return None;
    }
    if app.sidebar {
        return sidebar(frame, app, area, smooth);
    }
    let rows = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(2),
        Constraint::Min(8),
        Constraint::Length(2),
    ])
    .split(area);
    header(frame, app, rows[0]);
    let toolbar = Layout::horizontal([Constraint::Min(20), Constraint::Length(28)]).split(rows[1]);
    frame.render_widget(
        Paragraph::new(search_prompt(app, toolbar[0].width)).style(if app.searching {
            Style::default().fg(theme.accent())
        } else {
            theme.muted()
        }),
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
            app.repo.commit_count()
        ))
        .right_aligned()
        .style(theme.muted()),
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
    view
}

fn sidebar(frame: &mut Frame, app: &mut App, area: Rect, smooth: bool) -> Option<Viewport> {
    let theme = app.theme;
    let rows = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(4),
        Constraint::Length(2),
    ])
    .split(area);
    let name = app
        .repo
        .root
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| app.repo.root.display().to_string());
    frame.render_widget(
        Paragraph::new(vec![
            Line::styled(
                format!(" {}", clean(&name)),
                Style::default().fg(theme.fg()).add_modifier(Modifier::BOLD),
            ),
            Line::styled(
                format!(" {}", clean(&app.repo.head)),
                Style::default().fg(theme.accent()),
            ),
        ])
        .style(Style::default().bg(theme.panel())),
        rows[0],
    );
    let view = history(frame, app, rows[1], smooth);
    let count = app.repo.commits.len();
    let status = if app.searching || !app.query.is_empty() {
        search_prompt(app, rows[2].width)
    } else {
        elide(
            &format!(
                "{}/{}{} · {}",
                if count == 0 { 0 } else { app.selected + 1 },
                count,
                if app.repo.commit_count() == app.limit {
                    " (limit)"
                } else {
                    ""
                },
                clean(&app.status),
            ),
            rows[2].width,
            false,
        )
    };
    frame.render_widget(
        Paragraph::new(vec![
            Line::styled(status, theme.muted()),
            Line::styled(
                key_hints(app, rows[2].width),
                Style::default().fg(theme.accent()).bg(theme.panel()),
            ),
        ]),
        rows[2],
    );
    view
}

fn header(frame: &mut Frame, app: &App, area: Rect) {
    let theme = app.theme;
    let name = app
        .repo
        .root
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| app.repo.root.display().to_string());
    let lines = vec![
        Line::from(vec![
            Span::styled("  ◈  ", Style::default().fg(theme.accent())),
            Span::styled("HERDR ", theme.muted()),
            Span::styled(
                "GIT GRAPH",
                Style::default().fg(theme.fg()).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::styled(
                format!("     {}", clean(&name)),
                Style::default().fg(theme.fg()),
            ),
            Span::styled("  /  ", Style::default().fg(theme.border())),
            Span::styled(clean(&app.repo.head), Style::default().fg(theme.accent())),
        ]),
    ];
    frame.render_widget(
        Paragraph::new(lines).style(Style::default().bg(theme.panel())),
        area,
    );
}

fn branches(frame: &mut Frame, app: &mut App, area: Rect) {
    let theme = app.theme;
    let b = block(theme, "BRANCHES", app.focus == Focus::Branches);
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
                Style::default().fg(theme.accent()),
            ),
            Span::styled(
                name,
                if selected {
                    theme.selected_text().fg(theme.fg())
                } else {
                    theme.muted()
                },
            ),
        ])
        .style(Style::default().bg(if selected {
            theme.selection()
        } else {
            theme.bg()
        }))
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
                Line::from(format!(" {} refs · Enter apply", app.repo.branches.len())),
                Line::from(if app.refresh_interval.is_some() {
                    " Live · r reload"
                } else {
                    " Local · r reload"
                }),
            ])
            .style(theme.muted()),
            foot,
        );
    }
}

fn history(frame: &mut Frame, app: &mut App, area: Rect, smooth: bool) -> Option<Viewport> {
    let theme = app.theme;
    let b = block(
        theme,
        if app.sidebar && app.loading {
            "GIT GRAPH · loading…"
        } else if app.sidebar {
            "GIT GRAPH"
        } else if app.loading {
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
    let body = if app.sidebar {
        inner
    } else {
        Rect::new(inner.x, inner.y + 1, inner.width, inner.height - 1)
    };
    app.history_area = body;
    let count = usize::from(body.height).div_ceil(2).max(1);
    if app.selected < app.top {
        app.top = app.selected;
    }
    if app.selected >= app.top + count {
        app.top = app.selected + 1 - count;
    }
    let graph_width = ((app.graph.width * 3 + 2).max(if app.sidebar { 5 } else { 10 }) as u16)
        .min(36)
        .min(inner.width / 3);
    let cols = Layout::horizontal([
        Constraint::Length(graph_width),
        Constraint::Min(10),
        Constraint::Length(if !app.sidebar && inner.width >= 72 {
            19
        } else {
            0
        }),
    ])
    .spacing(1)
    .split(body);
    if !app.sidebar {
        let col_header = Rect::new(inner.x, inner.y, inner.width, 1);
        frame.render_widget(
            Paragraph::new(" GRAPH").style(theme.muted().bg(theme.panel())),
            col_header,
        );
        frame.render_widget(
            Paragraph::new("COMMIT / DESCRIPTION").style(theme.muted().bg(theme.panel())),
            Rect::new(cols[1].x, inner.y, cols[1].width, 1),
        );
        if cols[2].width > 0 {
            frame.render_widget(
                Paragraph::new("AUTHOR · DATE").style(theme.muted().bg(theme.panel())),
                Rect::new(cols[2].x, inner.y, cols[2].width, 1),
            );
        }
    }
    if app.repo.commits.is_empty() {
        frame.render_widget(
            Paragraph::new(if app.refresh_interval.is_some() {
                "\n No commits or changes yet.\n Watching for local changes…"
            } else {
                "\n No commits or changes yet.\n Make a change, then press r."
            })
            .style(theme.muted())
            .wrap(Wrap { trim: true }),
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
        let bg = if selected {
            theme.selection()
        } else {
            theme.bg()
        };
        frame.render_widget(
            Block::default().style(Style::default().bg(bg)),
            Rect::new(body.x, y, body.width, 1),
        );
        let short: String = if commit.is_worktree() {
            "WIP".into()
        } else {
            commit.oid.chars().take(7).collect()
        };
        let is_match = !app.query.is_empty()
            && [&commit.subject, &commit.author, &commit.refs, &commit.oid]
                .iter()
                .any(|s| s.to_lowercase().contains(&app.query.to_lowercase()));
        let mut spans = Vec::new();
        if !app.sidebar {
            spans.push(Span::styled(
                format!("{short}  "),
                Style::default().fg(theme.lane(app.graph.rows[i].color)),
            ));
        }
        spans.push(Span::styled(
            clean(&commit.subject),
            Style::default()
                .fg(if is_match {
                    theme.highlight()
                } else {
                    theme.fg()
                })
                .patch(if selected {
                    theme.selected_text()
                } else {
                    Style::default()
                }),
        ));
        let line = Line::from(spans);
        frame.render_widget(
            Paragraph::new(line),
            Rect::new(cols[1].x, y, cols[1].width, 1),
        );
        if y + 1 < body.bottom() {
            let mut refs = Vec::new();
            if app.sidebar {
                refs.push(Span::styled(format!("{short} "), theme.muted()));
            }
            refs.push(Span::styled(
                clean(&commit.refs),
                Style::default().fg(theme.accent()),
            ));
            frame.render_widget(
                Paragraph::new(Line::from(refs)),
                Rect::new(cols[1].x, y + 1, cols[1].width, 1),
            );
        }
        if cols[2].width > 0 {
            frame.render_widget(
                Paragraph::new(clean(&commit.author)).style(if selected {
                    Style::default().fg(theme.fg())
                } else {
                    theme.muted()
                }),
                Rect::new(cols[2].x, y, cols[2].width, 1),
            );
            if y + 1 < body.bottom() {
                frame.render_widget(
                    Paragraph::new(commit.date.clone()).style(theme.muted()),
                    Rect::new(cols[2].x, y + 1, cols[2].width, 1),
                );
            }
        }
        if !smooth {
            text_graph(
                frame,
                theme,
                &app.graph.rows[i],
                cols[0],
                y,
                app.graph_offset,
            );
        }
    }
    Some(Viewport {
        area: cols[0],
        top: app.top,
        selected: app.selected,
        column_offset: app.graph_offset,
    })
}

fn text_graph(
    frame: &mut Frame,
    theme: Theme,
    row: &crate::graph::Row,
    area: Rect,
    y: u16,
    offset: usize,
) {
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
            cell.set_symbol(symbol).set_fg(theme.lane(c));
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
    put(
        row.column * 3 + 1,
        y,
        if row.uncommitted { "○" } else { "●" },
        row.color,
        false,
    );
}

fn details(frame: &mut Frame, app: &mut App, area: Rect) {
    let theme = app.theme;
    let b = block(
        theme,
        if app.selected_oid() == Some(crate::git::WORKTREE_OID) {
            "WORKING TREE"
        } else {
            "COMMIT INSPECTOR"
        },
        app.focus == Focus::Details,
    );
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
                theme.added()
            } else if line.starts_with('-') && !line.starts_with("---") {
                theme.removed()
            } else if line.starts_with("@@") {
                theme.hunk()
            } else if line.starts_with("commit ") || line.starts_with("diff ") {
                theme.accent()
            } else {
                theme.muted().fg.unwrap_or(Color::Reset)
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
    let theme = app.theme;
    let range = if app.repo.commits.is_empty() {
        "0 / 0".into()
    } else {
        format!(
            "{} / {}{}",
            app.selected + 1,
            app.repo.commits.len(),
            if app.repo.commit_count() == app.limit {
                " · limit reached"
            } else {
                ""
            }
        )
    };
    let rows = Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).split(area);
    let cols = Layout::horizontal([
        Constraint::Min(10),
        Constraint::Length((Line::raw(&range).width() + 1) as u16),
    ])
    .split(rows[0]);
    frame.render_widget(
        Paragraph::new(elide(
            &format!(" {}", clean(&app.status)),
            cols[0].width,
            false,
        ))
        .style(theme.muted()),
        cols[0],
    );
    frame.render_widget(
        Paragraph::new(format!("{range} "))
            .right_aligned()
            .style(theme.muted()),
        cols[1],
    );
    frame.render_widget(
        Paragraph::new(key_hints(app, rows[1].width))
            .style(Style::default().bg(theme.panel()).fg(theme.accent())),
        rows[1],
    );
}

// Keep complete key/action pairs; the essential hints fit even at 24 columns.
fn key_hints(app: &App, width: u16) -> String {
    if app.searching {
        return "Enter apply · Esc clear".into();
    }
    let mut hints = vec!["/ search", "? help", "q quit"];
    let extras: &[&str] = if app.sidebar {
        &["↑↓ move", "r reload", "n/N matches", "h/l pan"]
    } else {
        &[
            "Tab panels",
            "↑↓ move",
            "d details",
            "r reload",
            "a all",
            "n/N matches",
        ]
    };
    for extra in extras {
        if Line::raw(hints.join("  ")).width() + 2 + Line::raw(*extra).width() <= usize::from(width)
        {
            hints.push(extra);
        }
    }
    hints.join("  ")
}

fn elide(text: &str, width: u16, keep_end: bool) -> String {
    let line = Line::raw(text);
    if line.width() <= usize::from(width) {
        return text.into();
    }
    if width == 0 {
        return String::new();
    }
    let mut graphemes: Vec<_> = line.styled_graphemes(Style::default()).collect();
    if keep_end {
        graphemes.reverse();
    }
    let mut used = 1; // Reserve an ellipsis, never half a wide character.
    let mut visible = Vec::new();
    for grapheme in graphemes {
        used += Span::raw(grapheme.symbol).width();
        if used > usize::from(width) {
            break;
        }
        visible.push(grapheme.symbol);
    }
    if keep_end {
        visible.reverse();
        format!("…{}", visible.concat())
    } else {
        format!("{}…", visible.concat())
    }
}

fn search_prompt(app: &App, width: u16) -> String {
    if app.searching {
        format!(
            "/ {}",
            elide(
                &format!("{}▏", clean(&app.query)),
                width.saturating_sub(2),
                true
            )
        )
    } else if !app.query.is_empty() {
        let hint = if app.status.starts_with("No matching") {
            " · no match"
        } else {
            " · n/N jump"
        };
        let available = width.saturating_sub(2 + Line::raw(hint).width() as u16);
        format!("/ {}{hint}", elide(&clean(&app.query), available, false))
    } else if width >= 43 {
        "/ Search commits, authors, refs or hashes".into()
    } else {
        "/ Search history".into()
    }
}

fn help(frame: &mut Frame, app: &mut App, area: Rect) {
    let theme = app.theme;
    let mut entries = vec![
        "↑↓ / j k: Move / scroll",
        "PgUp / PgDn: Page",
        "Home / g: First item",
        "End / G: Last item",
        "←→ / h l: Pan graph",
        "/: Search history",
        "n / N: Next / previous match",
        "Enter in search: Apply",
        "Esc: Clear search",
        "r: Reload repository",
        "a: Show all refs",
        "Mouse: Select / scroll",
        "q: Quit graph",
        "Ctrl-C: Quit anytime",
    ];
    if !app.sidebar {
        entries.extend([
            "Tab / Shift-Tab: Next / previous panel",
            "Enter in branches: Apply filter",
            "Enter in history: Open details",
            "d: Toggle details",
            "←→ / h l in details: Scroll diff",
        ]);
    }
    entries.extend([
        "Search covers loaded history and keeps graph nodes visible.",
        "In help: ↑↓ / j k scroll; PgUp / PgDn page; Home / End jump.",
    ]);
    let width = if app.sidebar {
        area.width
    } else {
        64.min(area.width - 2)
    };
    let content_width = usize::from(width.saturating_sub(2)).max(1);
    let mut lines = Vec::new();
    for entry in entries {
        let mut line = String::new();
        for word in entry.split_whitespace() {
            if !line.is_empty()
                && Line::raw(&line).width() + 1 + Line::raw(word).width() > content_width
            {
                lines.push(Line::raw(std::mem::take(&mut line)));
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
        lines.push(Line::raw(line));
    }
    let height = if app.sidebar {
        area.height
    } else {
        (lines.len() as u16 + 4).min(area.height - 2)
    };
    let modal = Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    );
    let border = block(
        theme,
        if app.sidebar {
            "Help"
        } else {
            "Keyboard shortcuts"
        },
        true,
    );
    let inner = border.inner(modal);
    frame.render_widget(Clear, modal);
    frame.render_widget(
        border.style(Style::default().fg(theme.fg()).bg(theme.panel())),
        modal,
    );
    let rows = Layout::vertical([
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .split(inner);
    app.help_page_size = usize::from(rows[0].height);
    app.help_scroll = app
        .help_scroll
        .min(lines.len().saturating_sub(app.help_page_size));
    let progress = format!("↑↓ scroll  {}/{}", app.help_scroll + 1, lines.len());
    frame.render_widget(
        Paragraph::new(lines).scroll((app.help_scroll as u16, 0)),
        rows[0],
    );
    frame.render_widget(Paragraph::new(progress).style(theme.muted()), rows[1]);
    frame.render_widget(
        Paragraph::new("Esc/?/q close help").style(Style::default().fg(theme.accent())),
        rows[2],
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
    let theme = app.theme;
    let (r, g, b) = theme.export_rgb(theme.bg(), true);
    let mut terminal = Terminal::new(TestBackend::new(width, height))?;
    let mut viewport = None;
    terminal.draw(|frame| {
        viewport = draw(frame, app, smooth);
    })?;
    let buffer = terminal.backend().buffer();
    let mut svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{}\" height=\"{}\" viewBox=\"0 0 {} {}\"><rect width=\"100%\" height=\"100%\" fill=\"#{r:02x}{g:02x}{b:02x}\"/><g font-family=\"Menlo, 'DejaVu Sans Mono', monospace\" font-size=\"14\">",
        u32::from(width) * 9,
        u32::from(height) * 20,
        u32::from(width) * 9,
        u32::from(height) * 20
    );
    for y in 0..height {
        for x in 0..width {
            let cell = &buffer[(x, y)];
            {
                let color = if cell.modifier.contains(Modifier::REVERSED) {
                    cell.fg
                } else {
                    cell.bg
                };
                let (r, g, b) =
                    theme.export_rgb(color, !cell.modifier.contains(Modifier::REVERSED));
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
            let reversed = cell.modifier.contains(Modifier::REVERSED);
            let (r, g, b) = theme.export_rgb(if reversed { cell.bg } else { cell.fg }, reversed);
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
        let png = crate::graphics::rasterize(&app.graph, view, (9, 20), theme)?.encode_png()?;
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
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn screen(app: &mut App, width: u16, height: u16) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| {
                draw(frame, app, false);
            })
            .unwrap();
        terminal
            .backend()
            .buffer()
            .content
            .chunks(usize::from(width))
            .map(|row| row.iter().map(|cell| cell.symbol()).collect())
            .collect()
    }

    #[test]
    fn essential_hints_and_search_cursor_fit_supported_widths() {
        for sidebar in [false, true] {
            let mut app = App::new(crate::git::demo(), 2000, true).with_sidebar(sidebar);
            for width in (if sidebar { 24 } else { 64 })..=150 {
                let height = if sidebar { 8 } else { 18 };
                let lines = screen(&mut app, width, height);
                let footer = lines.last().unwrap();
                for hint in ["/ search", "? help", "q quit"] {
                    assert!(footer.contains(hint), "{width}: {footer}");
                }
                assert!(Line::raw(key_hints(&app, width)).width() <= usize::from(width));
                app.query = "긴 검색어와 e\u{301} 👩‍💻 한글을 반복해서 입력합니다".repeat(10);
                app.searching = true;
                let lines = screen(&mut app, width, height);
                let query = &lines[if sidebar { usize::from(height) - 2 } else { 3 }];
                assert!(query.contains("/ …") && query.contains('▏'), "{query}");
                assert!(lines.last().unwrap().contains("Enter apply · Esc clear"));
                app.searching = false;
                let lines = screen(&mut app, width, height);
                assert!(lines.iter().any(|line| line.contains("n/N jump")));
                app.query.clear();
            }
        }
    }

    #[test]
    fn short_help_scrolls_to_every_instruction_and_keeps_close_visible() {
        for (sidebar, width, height) in [
            (true, 24, 8),
            (true, 26, 12),
            (false, 64, 18),
            (false, 140, 44),
        ] {
            let mut app = App::new(crate::git::demo(), 2000, true).with_sidebar(sidebar);
            let press =
                |app: &mut App, code| app.key(KeyEvent::new(code, KeyModifiers::NONE), None);
            press(&mut app, KeyCode::Char('?'));
            let mut visible = String::new();
            loop {
                let lines = screen(&mut app, width, height);
                let page = lines.join("\n");
                assert!(page.contains("Esc/?/q close help"), "{page}");
                assert!(page.contains("↑↓ scroll"));
                assert_eq!(app.history_area, Rect::default());
                visible.push_str(&page);
                let before = app.help_scroll;
                press(&mut app, KeyCode::PageDown);
                screen(&mut app, width, height);
                if app.help_scroll == before {
                    break;
                }
            }
            for instruction in [
                "PgUp / PgDn",
                "Home / g",
                "End / G",
                "n / N",
                "Enter in search",
                "Esc: Clear search",
                "r: Reload",
                "a: Show all refs",
                "Mouse:",
                "q: Quit graph",
                "Ctrl-C:",
                "nodes visible.",
                "In help:",
            ] {
                assert!(
                    visible.contains(instruction),
                    "missing {instruction} at {width}x{height}: {visible}"
                );
            }
            assert_eq!(visible.contains("Toggle details"), !sidebar);
            press(&mut app, KeyCode::Home);
            assert_eq!(app.help_scroll, 0);
            press(&mut app, KeyCode::End);
            screen(&mut app, width, height);
            press(&mut app, KeyCode::PageUp);
            assert_ne!(app.help_scroll, usize::MAX);
            press(&mut app, KeyCode::Char('q'));
            assert!(!app.help && !app.quit);
            assert_eq!(app.selected, 0);
            press(&mut app, KeyCode::Char('?'));
            assert_eq!(app.help_scroll, 0);
            app.key(
                KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
                None,
            );
            assert!(app.quit);
        }
    }

    #[test]
    fn terminal_theme_keeps_panel_defaults_and_a_visible_selection_without_queries() {
        for sidebar in [false, true] {
            let mut app = App::new(crate::git::demo(), 2000, true).with_sidebar(sidebar);
            let mut terminal = Terminal::new(TestBackend::new(140, 44)).unwrap();
            terminal
                .draw(|frame| {
                    draw(frame, &mut app, false);
                })
                .unwrap();
            let buffer = terminal.backend().buffer();
            assert!(buffer.content.iter().all(|cell| cell.bg == Color::Reset));
            assert!(
                buffer
                    .content
                    .iter()
                    .any(|cell| cell.modifier.contains(Modifier::REVERSED))
            );
            assert!(
                buffer
                    .content
                    .iter()
                    .any(|cell| cell.fg == Color::Indexed(6))
            );
        }
    }

    #[test]
    fn sidebar_renders_narrow_history_without_hidden_panels() {
        for (width, height) in [(24, 8), (32, 12), (48, 30), (140, 44)] {
            for smooth in [false, true] {
                let mut app = App::new(crate::git::demo(), 2000, true).with_sidebar(true);
                app.selected = 8;
                let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
                let mut view = None;
                terminal
                    .draw(|frame| {
                        view = draw(frame, &mut app, smooth);
                    })
                    .unwrap();
                let view = view.expect("Sidebar should fit in a narrow pane");
                assert!(view.area.width > 0 && view.area.height > 0);
                assert!(app.top <= app.selected);
                assert!(app.selected < app.top + usize::from(app.history_area.height).div_ceil(2));
                assert_eq!(app.branches_area, Rect::default());
                assert_eq!(app.detail_area, Rect::default());
                let screen: String = terminal
                    .backend()
                    .buffer()
                    .content
                    .iter()
                    .map(|c| c.symbol())
                    .collect();
                assert!(screen.contains("GIT GRAPH"));
                assert!(!screen.contains("BRANCHES"));
                assert!(!screen.contains("COMMIT INSPECTOR"));
                app.help = true;
                terminal
                    .draw(|frame| {
                        assert!(draw(frame, &mut app, smooth).is_none());
                    })
                    .unwrap();
                assert_eq!(app.history_area, Rect::default());
            }
        }
    }

    #[test]
    fn sidebar_empty_history_and_small_resize_clear_mouse_targets() {
        let mut repo = crate::git::demo();
        repo.commits.clear();
        let mut app = App::new(repo, 2000, true).with_sidebar(true);
        for (width, height) in [(32, 12), (20, 8)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal
                .draw(|frame| {
                    assert!(draw(frame, &mut app, true).is_none());
                })
                .unwrap();
        }
        assert_eq!(app.history_area, Rect::default());
        assert_eq!(app.branches_area, Rect::default());
        assert_eq!(app.detail_area, Rect::default());
    }

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
