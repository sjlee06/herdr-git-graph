use clap::ValueEnum;
use ratatui::style::{Color, Modifier, Style};

pub type Rgb = (u8, u8, u8);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub enum ThemeMode {
    /// Query the terminal's current colors; inherit defaults if queries are unsupported
    #[default]
    Auto,
    /// Inherit terminal defaults without sending color queries
    Terminal,
    /// Use the original dark Git Graph theme
    Classic,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Theme {
    pub mode: ThemeMode,
    pub foreground: Option<Rgb>,
    pub background: Option<Rgb>,
    // ANSI red, green, yellow, blue, magenta, cyan, in terminal palette order.
    pub palette: [Option<Rgb>; 6],
}

pub const CLASSIC_BG: Rgb = (12, 17, 24);
pub const CLASSIC_SELECTION: Rgb = (28, 46, 59);
const CLASSIC_FG: Rgb = (219, 229, 239);
const LANES: [usize; 6] = [5, 4, 2, 3, 0, 1];

fn rgb(c: Rgb) -> Color {
    Color::Rgb(c.0, c.1, c.2)
}

fn blend(bg: Rgb, fg: Rgb, percent: u16) -> Rgb {
    let channel =
        |a: u8, b: u8| ((u16::from(a) * (100 - percent) + u16::from(b) * percent) / 100) as u8;
    (
        channel(bg.0, fg.0),
        channel(bg.1, fg.1),
        channel(bg.2, fg.2),
    )
}

impl Theme {
    pub fn new(mode: ThemeMode) -> Self {
        Self {
            mode,
            ..Self::default()
        }
    }

    pub fn bg(self) -> Color {
        if self.mode == ThemeMode::Classic {
            rgb(CLASSIC_BG)
        } else {
            Color::Reset
        }
    }

    pub fn fg(self) -> Color {
        if self.mode == ThemeMode::Classic {
            rgb(CLASSIC_FG)
        } else {
            Color::Reset
        }
    }

    pub fn panel(self) -> Color {
        if self.mode == ThemeMode::Classic {
            Color::Rgb(15, 22, 31)
        } else {
            Color::Reset
        }
    }

    pub fn muted(self) -> Style {
        if self.mode == ThemeMode::Classic {
            Style::default().fg(Color::Rgb(117, 137, 158))
        } else if let (Some(bg), Some(fg)) = (self.background, self.foreground) {
            Style::default().fg(rgb(blend(bg, fg, 70)))
        } else {
            Style::default()
                .fg(Color::Reset)
                .add_modifier(Modifier::DIM)
        }
    }

    pub fn border(self) -> Color {
        if self.mode == ThemeMode::Classic {
            Color::Rgb(40, 55, 70)
        } else {
            self.muted().fg.unwrap_or(Color::Reset)
        }
    }

    pub fn selection_rgb(self) -> Option<Rgb> {
        if self.mode == ThemeMode::Classic {
            Some(CLASSIC_SELECTION)
        } else {
            Some(blend(self.background?, self.foreground?, 14))
        }
    }

    pub fn selection(self) -> Color {
        self.selection_rgb().map(rgb).unwrap_or(Color::Reset)
    }

    pub fn selected_text(self) -> Style {
        let style = Style::default().add_modifier(Modifier::BOLD);
        if self.selection_rgb().is_none() {
            style.add_modifier(Modifier::REVERSED)
        } else {
            style
        }
    }

    pub fn accent(self) -> Color {
        self.semantic(5)
    }
    pub fn added(self) -> Color {
        self.semantic(1)
    }
    pub fn removed(self) -> Color {
        self.semantic(0)
    }
    pub fn highlight(self) -> Color {
        self.semantic(2)
    }
    pub fn hunk(self) -> Color {
        self.semantic(4)
    }

    fn semantic(self, index: usize) -> Color {
        if self.mode == ThemeMode::Classic {
            return rgb([
                (251, 113, 133),
                (163, 230, 153),
                (251, 191, 106),
                (96, 165, 250),
                (167, 139, 250),
                (94, 234, 212),
            ][index]);
        }
        self.palette[index]
            .map(rgb)
            .unwrap_or(Color::Indexed(index as u8 + 1))
    }

    pub fn lane(self, index: usize) -> Color {
        if self.mode == ThemeMode::Classic {
            rgb(self.lane_rgb(index))
        } else {
            self.semantic(LANES[index % LANES.len()])
        }
    }

    pub fn lane_rgb(self, index: usize) -> Rgb {
        if self.mode != ThemeMode::Classic
            && let Some(color) = self.palette[LANES[index % LANES.len()]]
        {
            return color;
        }
        crate::graph::PALETTE[index % crate::graph::PALETTE.len()]
    }

    /// A PNG cannot represent unresolved ANSI palette entries.
    pub fn supports_curves(self) -> bool {
        self.mode == ThemeMode::Classic || self.palette.iter().all(Option::is_some)
    }

    /// Headless exports cannot query a terminal. Use a documented sample palette.
    pub fn export_rgb(self, color: Color, background: bool) -> Rgb {
        match color {
            Color::Rgb(r, g, b) => (r, g, b),
            Color::Indexed(i @ 1..=6) => self.palette[usize::from(i - 1)].unwrap_or(
                [
                    (205, 49, 49),
                    (13, 188, 121),
                    (229, 229, 16),
                    (36, 114, 200),
                    (188, 63, 188),
                    (17, 168, 205),
                ][usize::from(i - 1)],
            ),
            _ if background => self.background.unwrap_or(CLASSIC_BG),
            _ => self.foreground.unwrap_or(CLASSIC_FG),
        }
    }

    pub fn apply_response(&mut self, body: &str) {
        let mut fields = body.split(';');
        match fields.next() {
            Some("10") => {
                if let Some(color) = fields.next().and_then(parse_rgb) {
                    self.foreground = Some(color);
                }
            }
            Some("11") => {
                if let Some(color) = fields.next().and_then(parse_rgb) {
                    self.background = Some(color);
                }
            }
            Some("4") => {
                while let (Some(index), Some(value)) = (fields.next(), fields.next()) {
                    if let (Ok(index @ 1..=6), Some(color)) =
                        (index.parse::<usize>(), parse_rgb(value))
                    {
                        self.palette[index - 1] = Some(color);
                    }
                }
            }
            _ => {}
        }
    }
}

fn parse_rgb(value: &str) -> Option<Rgb> {
    let mut channels = value.strip_prefix("rgb:")?.split('/');
    let mut channel = || {
        let part = channels.next()?;
        if !(1..=4).contains(&part.len()) || !part.bytes().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        let value = u32::from_str_radix(part, 16).ok()?;
        Some((value * 255 / ((1 << (part.len() * 4)) - 1)) as u8)
    };
    let color = (channel()?, channel()?, channel()?);
    channels.next().is_none().then_some(color)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_xterm_precision_and_rejects_malformed_colors() {
        assert_eq!(parse_rgb("rgb:f/8/0"), Some((255, 136, 0)));
        assert_eq!(parse_rgb("rgb:1234/abcd/ffff"), Some((18, 171, 255)));
        for invalid in [
            "rgb:/00/00",
            "rgb:zz/00/00",
            "rgb:fffff/0/0",
            "rgb:0/0/0/0",
            "red",
        ] {
            assert_eq!(parse_rgb(invalid), None);
        }
    }

    #[test]
    fn light_and_dark_themes_keep_native_background_and_share_lane_colors() {
        for (bg, fg) in [
            ((250, 250, 250), (30, 30, 30)),
            ((20, 20, 20), (230, 230, 230)),
        ] {
            let mut theme = Theme {
                background: Some(bg),
                foreground: Some(fg),
                ..Theme::default()
            };
            theme.apply_response("4;1;rgb:aa/22/33;6;rgb:11/88/99");
            assert_eq!(theme.bg(), Color::Reset);
            assert_eq!(theme.fg(), Color::Reset);
            assert_eq!(theme.lane(0), Color::Rgb(17, 136, 153));
            assert_eq!(theme.lane_rgb(0), (17, 136, 153));
            assert!(theme.selection_rgb().unwrap().0 > bg.0.min(fg.0));
            assert!(theme.selection_rgb().unwrap().0 < bg.0.max(fg.0));
            theme.apply_response("11;rgb:invalid");
            assert_eq!(theme.background, Some(bg));
            assert!(!theme.supports_curves());
        }
    }
}
