use crate::{
    git::{self, Repository},
    graph::Graph,
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Focus {
    Branches,
    History,
    Details,
}

pub enum Request {
    Load {
        reference: Option<String>,
        id: u64,
    },
    Refresh {
        reference: Option<String>,
        id: u64,
        detail: bool,
    },
    Detail {
        oid: String,
        id: u64,
    },
}

pub enum Response {
    Loaded {
        result: Result<Repository, String>,
        reference: Option<String>,
        id: u64,
    },
    Detail {
        oid: String,
        id: u64,
        result: Result<String, String>,
    },
    Refreshed {
        result: Result<Repository, String>,
        detail: Option<Result<String, String>>,
        id: u64,
        elapsed: Duration,
    },
}

pub struct Worker {
    pub sender: mpsc::Sender<Request>,
    pub receiver: mpsc::Receiver<Response>,
}

impl Worker {
    pub fn start(root: PathBuf, limit: usize) -> Self {
        let (sender, requests) = mpsc::channel();
        let (responses, receiver) = mpsc::channel();
        thread::spawn(move || {
            let mut previous = None;
            while let Ok(request) = requests.recv() {
                // Coalesce within each request kind; a diff must never discard a refresh/load.
                let mut load = None;
                let mut refresh = None;
                let mut detail = None;
                for request in std::iter::once(request).chain(requests.try_iter()) {
                    match request {
                        Request::Load { .. } => load = Some(request),
                        Request::Refresh { .. } => refresh = Some(request),
                        Request::Detail { .. } => detail = Some(request),
                    }
                }
                for request in [load, refresh, detail].into_iter().flatten() {
                    let response = match request {
                        Request::Load { reference, id } => {
                            let result = git::load(&root, reference.as_deref(), limit)
                                .map_err(|e| format!("{e:#}"));
                            if let Ok(repo) = &result {
                                previous = Some(repo.clone());
                            }
                            Response::Loaded {
                                result,
                                reference,
                                id,
                            }
                        }
                        Request::Refresh {
                            reference,
                            id,
                            detail,
                        } => {
                            let start = Instant::now();
                            let result = match &previous {
                                Some(repo) => git::refresh(repo, reference.as_deref(), limit),
                                None => git::load(&root, reference.as_deref(), limit),
                            }
                            .map_err(|e| format!("{e:#}"));
                            let detail = result.as_ref().ok().filter(|_| detail).map(|repo| {
                                git::working_tree_details(&root, &repo.worktree)
                                    .map_err(|e| format!("{e:#}"))
                            });
                            if let Ok(repo) = &result {
                                previous = Some(repo.clone());
                            }
                            Response::Refreshed {
                                result,
                                detail,
                                id,
                                elapsed: start.elapsed(),
                            }
                        }
                        Request::Detail { oid, id } => Response::Detail {
                            result: git::details(&root, &oid).map_err(|e| format!("{e:#}")),
                            oid,
                            id,
                        },
                    };
                    if responses.send(response).is_err() {
                        return;
                    }
                }
            }
        });
        Self { sender, receiver }
    }
}

pub struct App {
    pub theme: crate::theme::Theme,
    pub repo: Repository,
    pub graph: Graph,
    pub selected: usize,
    pub top: usize,
    pub graph_offset: usize,
    pub branch_selected: usize,
    pub branch_top: usize,
    pub focus: Focus,
    pub query: String,
    pub searching: bool,
    pub help: bool,
    pub help_scroll: usize,
    pub help_page_size: usize,
    pub show_details: bool,
    pub sidebar: bool,
    pub detail: String,
    pub detail_scroll: usize,
    pub detail_horizontal: u16,
    pub detail_area: Rect,
    pub history_area: Rect,
    pub branches_area: Rect,
    pub status: String,
    pub renderer_status: String,
    pub reference: Option<String>,
    pub loading: bool,
    pub quit: bool,
    pub demo: bool,
    pub limit: usize,
    pub load_id: u64,
    pub refresh_interval: Option<Duration>,
    refresh_due: Instant,
    refreshing: bool,
    detail_due: Instant,
    requested_oid: Option<String>,
    cache: HashMap<String, String>,
}

impl App {
    pub fn new(repo: Repository, limit: usize, demo: bool) -> Self {
        let graph = Graph::build(&repo.commits);
        let mut app = Self {
            theme: crate::theme::Theme::default(),
            repo,
            graph,
            selected: 0,
            top: 0,
            graph_offset: 0,
            branch_selected: 0,
            branch_top: 0,
            focus: Focus::History,
            query: String::new(),
            searching: false,
            help: false,
            help_scroll: 0,
            help_page_size: 1,
            show_details: true,
            sidebar: false,
            detail: String::new(),
            detail_scroll: 0,
            detail_horizontal: 0,
            detail_area: Rect::default(),
            history_area: Rect::default(),
            branches_area: Rect::default(),
            status: if demo {
                "Demo · local Git history"
            } else {
                "Live · local Git history"
            }
            .into(),
            renderer_status: "TEXT".into(),
            reference: None,
            loading: false,
            quit: false,
            demo,
            limit,
            load_id: 0,
            refresh_interval: (!demo).then_some(Duration::from_secs(2)),
            refresh_due: Instant::now() + Duration::from_secs(2),
            refreshing: false,
            detail_due: Instant::now(),
            requested_oid: None,
            cache: HashMap::new(),
        };
        app.selection_changed();
        app
    }

    pub fn with_sidebar(mut self, sidebar: bool) -> Self {
        self.sidebar = sidebar;
        self.show_details = !sidebar;
        self.focus = Focus::History;
        self
    }

    pub fn selected_oid(&self) -> Option<&str> {
        self.repo.commits.get(self.selected).map(|c| c.oid.as_str())
    }

    pub fn with_refresh_interval(mut self, interval: Option<Duration>) -> Self {
        self.refresh_interval = interval.filter(|_| !self.demo);
        self.refresh_due = Instant::now() + interval.unwrap_or_default();
        if self.refresh_interval.is_none() && !self.demo {
            self.status = "Ready · manual refresh".into();
        }
        self
    }

    pub fn request_refresh(&mut self, worker: &Worker) {
        if self.demo
            || self.loading
            || self.refreshing
            || self.refresh_interval.is_none()
            || Instant::now() < self.refresh_due
        {
            return;
        }
        if worker
            .sender
            .send(Request::Refresh {
                reference: self.reference.clone(),
                id: self.load_id,
                detail: self.show_details && self.selected_oid() == Some(git::WORKTREE_OID),
            })
            .is_ok()
        {
            self.refreshing = true;
        }
    }

    pub fn selection_changed(&mut self) {
        self.detail_scroll = 0;
        self.detail_horizontal = 0;
        self.requested_oid = None;
        self.detail_due = Instant::now() + Duration::from_millis(100);
        self.detail = if self.demo {
            self.repo.commits.get(self.selected).map(|c| format!("commit {}\nAuthor: {}\nDate:   {}\n\n    {}\n\n src/graph.rs | 8 ++++++--\n 1 file changed, 6 insertions(+), 2 deletions(-)\n\ndiff --git a/src/graph.rs b/src/graph.rs\n--- a/src/graph.rs\n+++ b/src/graph.rs\n@@ -12,3 +12,7 @@\n-    draw_ascii_edge(from, to);\n+    let curve = BranchCurve::new(from, to);\n+    renderer.stroke(curve, color);\n+    renderer.draw_node(commit.position());\n\n[Demo data · no repository changes]",c.oid,c.author,c.date,c.subject)).unwrap_or_default()
        } else if let Some(oid) = self.selected_oid() {
            self.cache
                .get(oid)
                .cloned()
                .unwrap_or_else(|| "Loading commit…".into())
        } else {
            "No commits to inspect.".into()
        };
    }

    pub fn request_detail(&mut self, worker: &Worker) {
        if !self.show_details || self.demo || self.loading || Instant::now() < self.detail_due {
            return;
        }
        if let Some(oid) = self.selected_oid().map(str::to_owned)
            && !self.cache.contains_key(&oid)
            && self.requested_oid.as_ref() != Some(&oid)
        {
            let _ = worker.sender.send(Request::Detail {
                oid: oid.clone(),
                id: self.load_id,
            });
            self.requested_oid = Some(oid);
        }
    }

    pub fn apply(&mut self, response: Response) -> bool {
        match response {
            Response::Loaded {
                result,
                reference,
                id,
            } if id == self.load_id => {
                self.loading = false;
                match result {
                    Ok(repo) => {
                        let old = self.selected_oid().map(str::to_owned);
                        self.repo = repo;
                        self.graph = Graph::build(&self.repo.commits);
                        self.reference = reference;
                        self.selected = old
                            .and_then(|oid| self.repo.commits.iter().position(|c| c.oid == oid))
                            .unwrap_or(0);
                        self.branch_selected = self
                            .reference
                            .as_ref()
                            .and_then(|r| self.repo.branches.iter().position(|b| &b.reference == r))
                            .map(|i| i + 1)
                            .unwrap_or(0);
                        self.top = 0;
                        self.graph_offset = 0;
                        self.status = "History refreshed".into();
                        self.cache.clear();
                        self.selection_changed();
                    }
                    Err(error) => self.status = format!("Error: {error}"),
                }
            }
            Response::Refreshed {
                result,
                detail,
                id,
                elapsed,
            } => {
                self.refreshing = false;
                // Slow status/diff scans automatically get more idle time; never queue polls.
                self.refresh_due = Instant::now()
                    + self
                        .refresh_interval
                        .unwrap_or(Duration::from_secs(2))
                        .max(elapsed.saturating_mul(4));
                if id != self.load_id || self.loading {
                    return false;
                }
                let repo = match result {
                    Ok(repo) => repo,
                    Err(error) => {
                        self.refresh_due =
                            Instant::now() + Duration::from_secs(10).max(elapsed.saturating_mul(4));
                        let status = format!("Auto-refresh failed (will retry): {error}");
                        let changed = self.status != status;
                        self.status = status;
                        return changed;
                    }
                };
                let mut changed = self.repo != repo;
                if changed {
                    let old = self.selected_oid().map(str::to_owned);
                    let old_top = self.top;
                    let old_selected = self.selected;
                    let branch = self
                        .branch_selected
                        .checked_sub(1)
                        .and_then(|i| self.repo.branches.get(i))
                        .map(|b| b.reference.clone());
                    self.repo = repo;
                    self.graph = Graph::build(&self.repo.commits);
                    self.selected = old
                        .as_ref()
                        .and_then(|oid| self.repo.commits.iter().position(|c| &c.oid == oid))
                        .or_else(|| {
                            self.repo
                                .head_oid
                                .as_ref()
                                .filter(|_| old.as_deref() == Some(git::WORKTREE_OID))
                                .and_then(|oid| {
                                    self.repo.commits.iter().position(|c| &c.oid == oid)
                                })
                        })
                        .unwrap_or_else(|| {
                            old_selected.min(self.repo.commits.len().saturating_sub(1))
                        });
                    self.top = if old_top == 0 {
                        0
                    } else {
                        old_top
                            .saturating_add_signed(self.selected as isize - old_selected as isize)
                    };
                    self.branch_selected = branch
                        .and_then(|r| self.repo.branches.iter().position(|b| b.reference == r))
                        .map(|i| i + 1)
                        .unwrap_or(0);
                    if self.selected_oid() != old.as_deref() {
                        self.selection_changed();
                    }
                }
                if self.selected_oid() == Some(git::WORKTREE_OID) {
                    if let Some(detail) = detail {
                        let text = detail.unwrap_or_else(|e| format!("Cannot load changes: {e}"));
                        changed |= self.detail != text;
                        self.detail = text;
                        self.detail_scroll = self
                            .detail_scroll
                            .min(self.detail.lines().count().saturating_sub(1));
                        self.requested_oid = Some(git::WORKTREE_OID.into());
                    } else if changed {
                        self.requested_oid = None;
                    }
                }
                if changed || self.status.starts_with("Auto-refresh failed") {
                    self.status = "Live · repository updated".into();
                    return true;
                }
                return false;
            }
            Response::Detail { oid, id, result } if id == self.load_id => {
                let text = result.unwrap_or_else(|e| format!("Cannot load commit: {e}"));
                if self.selected_oid() == Some(&oid) {
                    self.detail = text.clone();
                }
                if oid == git::WORKTREE_OID {
                    return true;
                }
                if self.cache.len() >= 64 {
                    self.cache.clear();
                }
                self.cache.insert(oid, text);
            }
            _ => return false,
        }
        true
    }

    pub fn load(&mut self, reference: Option<String>, worker: Option<&Worker>) {
        if self.demo {
            self.status = "Demo · run with --repo to filter real branches".into();
            return;
        }
        if let Some(worker) = worker {
            self.load_id += 1;
            self.loading = true;
            self.status = "Loading history…".into();
            let _ = worker.sender.send(Request::Load {
                reference,
                id: self.load_id,
            });
        }
    }

    fn navigate(&mut self, delta: isize) {
        match self.focus {
            Focus::History => {
                self.selected = self
                    .selected
                    .saturating_add_signed(delta)
                    .min(self.repo.commits.len().saturating_sub(1));
                self.selection_changed();
            }
            Focus::Branches => {
                self.branch_selected = self
                    .branch_selected
                    .saturating_add_signed(delta)
                    .min(self.repo.branches.len())
            }
            Focus::Details => {
                self.detail_scroll = self
                    .detail_scroll
                    .saturating_add_signed(delta)
                    .min(self.detail.lines().count().saturating_sub(1))
            }
        }
    }

    pub fn find(&mut self, backwards: bool, include_current: bool) {
        let count = self.repo.commits.len();
        if count == 0 || self.query.is_empty() {
            return;
        }
        let query = self.query.to_lowercase();
        let start = usize::from(!include_current);
        for step in start..(count + start) {
            let index = if backwards {
                (self.selected + count - step % count) % count
            } else {
                (self.selected + step) % count
            };
            let c = &self.repo.commits[index];
            if [&c.oid, &c.author, &c.subject, &c.refs]
                .iter()
                .any(|s| s.to_lowercase().contains(&query))
            {
                self.selected = index;
                self.selection_changed();
                self.status = format!("Match {}/{} · n/N for next/previous", index + 1, count);
                return;
            }
        }
        self.status = "No matching commits in loaded history".into();
    }

    pub fn key(&mut self, key: KeyEvent, worker: Option<&Worker>) {
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.quit = true;
            return;
        }
        if self.help {
            match key.code {
                KeyCode::Esc | KeyCode::Char('?') | KeyCode::Char('q') => self.help = false,
                KeyCode::Down | KeyCode::Char('j') => {
                    self.help_scroll = self.help_scroll.saturating_add(1)
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    self.help_scroll = self.help_scroll.saturating_sub(1)
                }
                KeyCode::PageDown => {
                    self.help_scroll = self.help_scroll.saturating_add(self.help_page_size)
                }
                KeyCode::PageUp => {
                    self.help_scroll = self.help_scroll.saturating_sub(self.help_page_size)
                }
                KeyCode::Home | KeyCode::Char('g') => self.help_scroll = 0,
                KeyCode::End | KeyCode::Char('G') => self.help_scroll = usize::MAX,
                _ => {}
            }
            return;
        }
        if self.searching {
            match key.code {
                KeyCode::Esc => {
                    self.searching = false;
                    self.query.clear();
                }
                KeyCode::Enter => {
                    self.searching = false;
                    self.find(false, true);
                }
                KeyCode::Backspace => {
                    self.query.pop();
                    self.find(false, true);
                }
                KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.query.push(c);
                    self.find(false, true);
                }
                _ => {}
            }
            return;
        }
        match key.code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Char('?') => {
                self.help = true;
                self.help_scroll = 0;
            }
            KeyCode::Tab | KeyCode::BackTab | KeyCode::Enter | KeyCode::Char('d')
                if self.sidebar => {}
            KeyCode::Tab => {
                self.focus = match self.focus {
                    Focus::Branches => Focus::History,
                    Focus::History if self.show_details => Focus::Details,
                    _ => Focus::Branches,
                }
            }
            KeyCode::BackTab => {
                self.focus = match self.focus {
                    Focus::Details => Focus::History,
                    Focus::History => Focus::Branches,
                    _ if self.show_details => Focus::Details,
                    _ => Focus::History,
                }
            }
            KeyCode::Char('/') => {
                self.searching = true;
                self.focus = Focus::History;
                self.query.clear();
            }
            KeyCode::Char('n') => self.find(false, false),
            KeyCode::Char('N') => self.find(true, false),
            KeyCode::Char('r') => self.load(self.reference.clone(), worker),
            KeyCode::Char('a') => {
                self.branch_selected = 0;
                self.load(None, worker);
            }
            KeyCode::Char('d') => {
                self.show_details = !self.show_details;
                self.focus = Focus::History;
            }
            KeyCode::Esc => {
                self.query.clear();
                self.focus = Focus::History;
            }
            KeyCode::Enter if self.focus == Focus::Branches => {
                let reference = self
                    .branch_selected
                    .checked_sub(1)
                    .and_then(|i| self.repo.branches.get(i))
                    .map(|b| b.reference.clone());
                self.load(reference, worker);
                self.focus = Focus::History;
            }
            KeyCode::Enter => {
                self.show_details = true;
                self.focus = Focus::Details;
            }
            _ if self.loading => {}
            KeyCode::Down | KeyCode::Char('j') => self.navigate(1),
            KeyCode::Up | KeyCode::Char('k') => self.navigate(-1),
            KeyCode::PageDown => self.navigate(self.page_size() as isize),
            KeyCode::PageUp => self.navigate(-(self.page_size() as isize)),
            KeyCode::Home | KeyCode::Char('g') => self.navigate(-1_000_000),
            KeyCode::End | KeyCode::Char('G') => self.navigate(1_000_000),
            KeyCode::Left | KeyCode::Char('h') if self.focus == Focus::History => {
                self.graph_offset = self.graph_offset.saturating_sub(3)
            }
            KeyCode::Right | KeyCode::Char('l') if self.focus == Focus::History => {
                self.graph_offset =
                    (self.graph_offset + 3).min(self.graph.width.saturating_sub(1) * 3)
            }
            KeyCode::Left | KeyCode::Char('h') if self.focus == Focus::Details => {
                self.detail_horizontal = self.detail_horizontal.saturating_sub(4)
            }
            KeyCode::Right | KeyCode::Char('l') if self.focus == Focus::Details => {
                self.detail_horizontal = self.detail_horizontal.saturating_add(4)
            }
            _ => {}
        }
    }

    fn page_size(&self) -> usize {
        if self.focus == Focus::Details {
            usize::from(self.detail_area.height.saturating_sub(2)).max(1)
        } else {
            usize::from(self.history_area.height / 2).max(1)
        }
    }

    pub fn mouse(&mut self, event: MouseEvent, worker: Option<&Worker>) {
        if self.help || self.searching || self.loading {
            return;
        }
        let point = (event.column, event.row).into();
        if self.history_area.contains(point) {
            self.focus = Focus::History;
        } else if self.branches_area.contains(point) {
            self.focus = Focus::Branches;
        } else if self.detail_area.contains(point) {
            self.focus = Focus::Details;
        } else {
            return;
        }
        match event.kind {
            MouseEventKind::ScrollDown => self.navigate(3),
            MouseEventKind::ScrollUp => self.navigate(-3),
            MouseEventKind::Down(crossterm::event::MouseButton::Left) => match self.focus {
                Focus::History => {
                    self.selected = (self.top
                        + usize::from(event.row.saturating_sub(self.history_area.y)) / 2)
                        .min(self.repo.commits.len().saturating_sub(1));
                    self.selection_changed();
                }
                Focus::Branches => {
                    let row = usize::from(event.row.saturating_sub(self.branches_area.y));
                    if row == 0 {
                        self.branch_selected = 0;
                    } else {
                        self.branch_selected =
                            (self.branch_top + row).min(self.repo.branches.len());
                    }
                    self.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), worker);
                }
                _ => {}
            },
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_worktree(mut repo: Repository) -> Repository {
        repo.commits.insert(
            0,
            git::Commit {
                oid: git::WORKTREE_OID.into(),
                parents: repo.head_oid.iter().cloned().collect(),
                subject: "Uncommitted changes".into(),
                refs: "1 unstaged".into(),
                author: "Working tree".into(),
                date: String::new(),
            },
        );
        repo
    }

    fn refreshed(repo: Repository, detail: Option<&str>) -> Response {
        Response::Refreshed {
            result: Ok(repo),
            detail: detail.map(|s| Ok(s.into())),
            id: 0,
            elapsed: Duration::ZERO,
        }
    }

    #[test]
    fn background_refresh_preserves_selection_scroll_and_reveals_worktree_at_top() {
        let mut app = App::new(git::demo(), 2000, false);
        app.detail = "existing commit diff".into();
        app.detail_scroll = 4;
        app.graph_offset = 3;
        let oid = app.selected_oid().unwrap().to_owned();
        let changed = with_worktree(app.repo.clone());
        assert!(app.apply(refreshed(changed, None)));
        assert_eq!(app.selected_oid(), Some(oid.as_str()));
        assert_eq!(app.selected, 1);
        assert_eq!(app.top, 0);
        assert_eq!(app.graph_offset, 3);
        assert_eq!(app.detail_scroll, 4);
        assert_eq!(app.detail, "existing commit diff");

        app.selected = 7;
        app.top = 5;
        let oid = app.selected_oid().unwrap().to_owned();
        assert!(app.apply(refreshed(git::demo(), None)));
        assert_eq!(app.selected_oid(), Some(oid.as_str()));
        assert_eq!(app.top, 4);
        assert_eq!(app.selected - app.top, 2);
    }

    #[test]
    fn unchanged_refresh_does_not_redraw_and_mutable_diff_is_not_cached() {
        let mut app = App::new(with_worktree(git::demo()), 2000, false);
        app.detail = "old\nsecond line\n".into();
        app.detail_scroll = 1;
        assert!(!app.apply(refreshed(app.repo.clone(), Some("old\nsecond line\n"))));
        assert!(app.apply(refreshed(app.repo.clone(), Some("new\nsecond line\n"))));
        assert_eq!(app.detail_scroll, 1);
        assert!(app.detail.starts_with("new"));
        assert!(!app.cache.contains_key(git::WORKTREE_OID));
        app.apply(Response::Detail {
            oid: git::WORKTREE_OID.into(),
            id: 0,
            result: Ok("fresh diff".into()),
        });
        assert!(!app.cache.contains_key(git::WORKTREE_OID));
        app.apply(refreshed(git::demo(), None));
        assert_eq!(app.selected_oid(), Some("a31c9f0"));
    }

    #[test]
    fn refresh_is_single_flight_nonblocking_and_disabled_in_manual_mode() {
        let mut app = App::new(git::demo(), 2000, false).with_sidebar(true);
        let (sender, requests) = mpsc::channel();
        let (_, receiver) = mpsc::channel();
        let worker = Worker { sender, receiver };
        app.refresh_due = Instant::now();
        app.request_refresh(&worker);
        assert!(matches!(
            requests.try_recv(),
            Ok(Request::Refresh { detail: false, .. })
        ));
        assert!(!app.loading);
        app.request_refresh(&worker);
        assert!(requests.try_recv().is_err());
        app.key(
            KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE),
            Some(&worker),
        );
        assert_eq!(app.selected, 1);
        app.apply(Response::Refreshed {
            result: Ok(app.repo.clone()),
            detail: None,
            id: 0,
            elapsed: Duration::from_secs(3),
        });
        assert!(app.refresh_due > Instant::now() + Duration::from_secs(11));
        app.refresh_due = Instant::now();
        app.refresh_interval = None;
        app.request_refresh(&worker);
        assert!(requests.try_recv().is_err());
        app.key(
            KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE),
            Some(&worker),
        );
        assert!(matches!(requests.try_recv(), Ok(Request::Load { .. })));
    }

    #[test]
    fn stale_refresh_and_diff_cannot_overwrite_new_branch_view() {
        let mut app = App::new(git::demo(), 2000, false);
        app.load_id = 2;
        app.detail = "current detail".into();
        assert!(!app.apply(refreshed(with_worktree(git::demo()), None)));
        assert!(!app.repo.commits[0].is_worktree());
        assert!(!app.apply(Response::Detail {
            oid: app.selected_oid().unwrap().into(),
            id: 1,
            result: Ok("stale detail".into())
        }));
        assert_eq!(app.detail, "current detail");
    }

    #[test]
    fn refresh_failure_keeps_history_and_recovers_without_user_input() {
        let mut app = App::new(git::demo(), 2000, false);
        app.apply(Response::Refreshed {
            result: Err("repository temporarily unavailable".into()),
            detail: None,
            id: 0,
            elapsed: Duration::ZERO,
        });
        assert_eq!(app.repo.commit_count(), 10);
        assert!(app.status.starts_with("Auto-refresh failed"));
        assert!(app.apply(refreshed(app.repo.clone(), None)));
        assert!(!app.status.contains("failed"));
    }

    #[test]
    fn sidebar_keeps_navigation_in_history_and_skips_diff_reads() {
        let mut app = App::new(git::demo(), 2000, false).with_sidebar(true);
        for code in [
            KeyCode::Tab,
            KeyCode::BackTab,
            KeyCode::Enter,
            KeyCode::Char('d'),
        ] {
            app.key(KeyEvent::new(code, KeyModifiers::NONE), None);
            assert_eq!(app.focus, Focus::History);
            assert!(!app.show_details);
        }
        app.key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE), None);
        assert_eq!(app.selected, 1);
        let (sender, requests) = mpsc::channel();
        let (_, receiver) = mpsc::channel();
        let worker = Worker { sender, receiver };
        app.detail_due = Instant::now();
        app.request_detail(&worker);
        assert!(requests.try_recv().is_err());
        app.query = "한글".into();
        app.find(false, true);
        assert_eq!(app.selected, 8);
        app.load(None, Some(&worker));
        assert!(matches!(requests.try_recv(), Ok(Request::Load { .. })));
    }

    #[test]
    fn unicode_search_keeps_graph_topology_and_wraps() {
        let mut app = App::new(git::demo(), 2000, true);
        app.query = "한글".into();
        app.find(false, true);
        assert_eq!(app.selected, 8);
        assert_eq!(app.graph.rows.len(), 10);
        app.find(false, false);
        assert_eq!(app.selected, 8);
    }
    #[test]
    fn stale_branch_response_cannot_replace_newer_view() {
        let mut app = App::new(git::demo(), 2000, false);
        app.load_id = 2;
        app.apply(Response::Loaded {
            result: Err("stale error".into()),
            reference: None,
            id: 1,
        });
        assert!(!app.status.contains("stale"));
    }
}
