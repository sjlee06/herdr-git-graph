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
    Load { reference: Option<String>, id: u64 },
    Detail { oid: String },
}

pub enum Response {
    Loaded {
        result: Result<Repository, String>,
        reference: Option<String>,
        id: u64,
    },
    Detail {
        oid: String,
        result: Result<String, String>,
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
            while let Ok(mut request) = requests.recv() {
                // Coalesce queued navigation requests to keep rapid key repeats responsive.
                while let Ok(newer) = requests.try_recv() {
                    request = newer;
                }
                let response = match request {
                    Request::Load { reference, id } => Response::Loaded {
                        result: git::load(&root, reference.as_deref(), limit)
                            .map_err(|e| format!("{e:#}")),
                        reference,
                        id,
                    },
                    Request::Detail { oid } => Response::Detail {
                        result: git::details(&root, &oid).map_err(|e| format!("{e:#}")),
                        oid,
                    },
                };
                if responses.send(response).is_err() {
                    break;
                }
            }
        });
        Self { sender, receiver }
    }
}

pub struct App {
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
    pub show_details: bool,
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
    detail_due: Instant,
    requested_oid: Option<String>,
    cache: HashMap<String, String>,
}

impl App {
    pub fn new(repo: Repository, limit: usize, demo: bool) -> Self {
        let graph = Graph::build(&repo.commits);
        let mut app = Self {
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
            show_details: true,
            detail: String::new(),
            detail_scroll: 0,
            detail_horizontal: 0,
            detail_area: Rect::default(),
            history_area: Rect::default(),
            branches_area: Rect::default(),
            status: "Ready · local Git history".into(),
            renderer_status: "TEXT".into(),
            reference: None,
            loading: false,
            quit: false,
            demo,
            limit,
            load_id: 0,
            detail_due: Instant::now(),
            requested_oid: None,
            cache: HashMap::new(),
        };
        app.selection_changed();
        app
    }

    pub fn selected_oid(&self) -> Option<&str> {
        self.repo.commits.get(self.selected).map(|c| c.oid.as_str())
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
        if self.demo || self.loading || Instant::now() < self.detail_due {
            return;
        }
        if let Some(oid) = self.selected_oid().map(str::to_owned)
            && !self.cache.contains_key(&oid)
            && self.requested_oid.as_ref() != Some(&oid)
        {
            let _ = worker.sender.send(Request::Detail { oid: oid.clone() });
            self.requested_oid = Some(oid);
        }
    }

    pub fn apply(&mut self, response: Response) {
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
            Response::Detail { oid, result } => {
                let text = result.unwrap_or_else(|e| format!("Cannot load commit: {e}"));
                if self.selected_oid() == Some(&oid) {
                    self.detail = text.clone();
                }
                if self.cache.len() >= 64 {
                    self.cache.clear();
                }
                self.cache.insert(oid, text);
            }
            _ => {}
        }
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
            if matches!(
                key.code,
                KeyCode::Esc | KeyCode::Char('?') | KeyCode::Char('q')
            ) {
                self.help = false;
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
            KeyCode::Char('?') => self.help = true,
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
