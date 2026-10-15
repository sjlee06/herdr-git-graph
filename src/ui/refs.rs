use super::{clean, elide};
use crate::{
    git::{Branch, Commit},
    theme::Theme,
};
use ratatui::{
    style::Style,
    text::{Line, Span},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Kind {
    Current,
    Local,
    Tag,
    Remote,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct Badge {
    name: String,
    kind: Kind,
}

impl Badge {
    fn label(&self) -> String {
        clean(&match self.kind {
            Kind::Current => format!("● {}", self.name),
            Kind::Tag => format!("tag: {}", self.name),
            _ => self.name.clone(),
        })
    }

    fn style(&self, theme: Theme) -> Style {
        let color = match self.kind {
            Kind::Current | Kind::Local => theme.accent(),
            Kind::Tag => theme.highlight(),
            Kind::Remote => theme.hunk(),
        };
        theme.badge(color, self.kind == Kind::Current)
    }
}

pub(super) fn badges(commit: &Commit, branches: &[Branch]) -> Vec<Badge> {
    if commit.is_worktree() {
        return Vec::new();
    }
    let mut badges: Vec<_> = commit
        .refs
        .split(", ")
        .filter(|name| !name.is_empty())
        .map(|name| {
            let (name, kind) = if let Some(name) = name.strip_prefix("HEAD -> ") {
                (name, Kind::Current)
            } else if name == "HEAD" {
                ("HEAD", Kind::Current)
            } else if let Some(name) = name.strip_prefix("tag: ") {
                (name, Kind::Tag)
            } else if let Some(branch) = branches.iter().find(|branch| {
                branch.name == name || branch.reference.strip_prefix("refs/") == Some(name)
            }) {
                (
                    branch.name.as_str(),
                    if branch.reference.starts_with("refs/remotes/") {
                        Kind::Remote
                    } else {
                        Kind::Local
                    },
                )
            } else if name.ends_with("/HEAD") {
                (name, Kind::Remote)
            } else {
                (name, Kind::Local)
            };
            Badge {
                name: name.into(),
                kind,
            }
        })
        .collect();
    badges.sort_by_key(|badge| badge.kind);
    badges
}

pub(super) fn line(badges: &[Badge], width: u16, theme: Theme) -> Line<'static> {
    let mut spans = Vec::new();
    let mut used = 0;
    let mut displayed = 0;
    for (i, badge) in badges.iter().enumerate() {
        let label = badge.label();
        let gap = usize::from(i > 0);
        let remaining = badges.len() - i - 1;
        let reserve = if remaining > 0 {
            Line::raw(format!(" +{remaining}")).width()
        } else {
            0
        };
        let available = usize::from(width).saturating_sub(used + gap + reserve);
        let fits = Line::raw(&label).width() + 2 <= available;
        if !fits && i > 0 {
            break;
        }
        if available < 3 {
            break;
        }
        if gap > 0 {
            spans.push(Span::raw(" "));
            used += 1;
        }
        let text = format!(" {} ", elide(&label, (available - 2) as u16, false));
        used += Line::raw(&text).width();
        spans.push(Span::styled(text, badge.style(theme)));
        displayed += 1;
        if !fits {
            break;
        }
    }
    if displayed < badges.len() {
        spans.push(Span::styled(
            format!(
                "{}+{}",
                if displayed > 0 { " " } else { "" },
                badges.len() - displayed
            ),
            theme.muted(),
        ));
    }
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refs_are_prioritized_without_hiding_colocated_remotes_or_remote_head() {
        let mut repo = crate::git::demo();
        repo.branches.push(Branch {
            reference: "refs/remotes/origin/feature/search".into(),
            name: "origin/feature/search".into(),
            oid: repo.commits[0].oid.clone(),
        });
        repo.commits[0].refs = "origin/feature/search, tag: v1.0, origin/HEAD, origin/main, feature/local, HEAD -> main".into();
        let labels = badges(&repo.commits[0], &repo.branches);
        assert_eq!(
            labels
                .iter()
                .map(|badge| (badge.name.as_str(), badge.kind))
                .collect::<Vec<_>>(),
            vec![
                ("main", Kind::Current),
                ("feature/local", Kind::Local),
                ("v1.0", Kind::Tag),
                ("origin/feature/search", Kind::Remote),
                ("origin/HEAD", Kind::Remote),
                ("origin/main", Kind::Remote),
            ]
        );
        repo.commits[0].refs = "origin/main".into();
        assert_eq!(
            badges(&repo.commits[0], &repo.branches)[0].kind,
            Kind::Remote
        );
    }

    #[test]
    fn remote_badges_use_the_magenta_palette_in_every_theme() {
        use crate::theme::ThemeMode;
        use ratatui::style::{Color, Modifier};

        let native = |bg, fg| Theme {
            background: Some(bg),
            foreground: Some(fg),
            palette: [
                None,
                None,
                Some((190, 140, 30)),
                None,
                Some((150, 80, 200)),
                Some((20, 150, 150)),
            ],
            ..Theme::default()
        };
        for theme in [
            Theme::new(ThemeMode::Classic),
            Theme::new(ThemeMode::Terminal),
            native((250, 250, 250), (30, 30, 30)),
            native((20, 20, 20), (230, 230, 230)),
        ] {
            let remote = Badge {
                name: "origin/main".into(),
                kind: Kind::Remote,
            }
            .style(theme);
            assert_eq!(remote, theme.badge(theme.hunk(), false));
            assert_ne!(remote, theme.badge(theme.accent(), false));
            assert_ne!(remote, theme.badge(theme.highlight(), false));
            assert!(!remote.add_modifier.contains(Modifier::DIM));
            assert_ne!(remote.fg, Some(Color::Reset));
        }
    }

    #[test]
    fn detached_head_and_worktree_are_not_misclassified() {
        let mut repo = crate::git::demo();
        repo.commits[0].refs = "tag: v1, HEAD".into();
        let labels = badges(&repo.commits[0], &repo.branches);
        assert_eq!(labels[0].label(), "● HEAD");
        assert_eq!(labels[1].label(), "tag: v1");
        repo.commits[0].oid = crate::git::WORKTREE_OID.into();
        repo.commits[0].refs = "2 staged · 1 untracked".into();
        assert!(badges(&repo.commits[0], &repo.branches).is_empty());
    }

    #[test]
    fn overflow_keeps_the_current_branch_and_counts_hidden_badges() {
        let badges = vec![
            Badge {
                name: "한글e\u{301}👩‍💻/very-long-branch".into(),
                kind: Kind::Current,
            },
            Badge {
                name: "v1.0".into(),
                kind: Kind::Tag,
            },
            Badge {
                name: "origin/other".into(),
                kind: Kind::Remote,
            },
        ];
        for width in 10..=80 {
            let line = line(&badges, width, Theme::default());
            assert!(line.width() <= usize::from(width), "{width}: {line}");
            assert!(line.to_string().contains('●'));
        }
        let narrow = line(&badges, 16, Theme::default()).to_string();
        assert!(narrow.contains('…') && narrow.ends_with(" +2"), "{narrow}");
        let wide = line(&badges, 80, Theme::default()).to_string();
        assert!(wide.contains("tag: v1.0") && wide.contains("origin/other"));
        assert!(!wide.contains("+2"));
    }
}
