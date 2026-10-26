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
    Files,
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
    Files {
        oid: String,
        id: u64,
    },
    FileDetail {
        oid: String,
        file: git::ChangedFile,
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
    Files {
        oid: String,
        id: u64,
        result: Result<Vec<git::ChangedFile>, String>,
    },
    FileDetail {
        oid: String,
        file: git::ChangedFile,
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
                let mut files = None;
                let mut file_detail = None;
                for request in std::iter::once(request).chain(requests.try_iter()) {
                    match request {
                        Request::Load { .. } => load = Some(request),
                        Request::Refresh { .. } => refresh = Some(request),
                        Request::Detail { .. } => detail = Some(request),
                        Request::Files { .. } => files = Some(request),
                        Request::FileDetail { .. } => file_detail = Some(request),
                    }
                }
                for request in [load, refresh, detail, files, file_detail]
                    .into_iter()
                    .flatten()
                {
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
                        Request::Files { oid, id } => Response::Files {
                            result: git::changed_files(&root, &oid).map_err(|e| format!("{e:#}")),
                            oid,
                            id,
                        },
                        Request::FileDetail { oid, file, id } => Response::FileDetail {
                            result: git::file_details(&root, &oid, &file)
                                .map_err(|e| format!("{e:#}")),
                            oid,
                            file,
                            id,
                        },
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
    pub selected: Option<usize>,
    pub top: usize,
    pub top_offset: usize,
    pub history_rows: Vec<Option<usize>>,
    pub history_file_rows: Vec<Option<usize>>,
    pub file_selected: Option<usize>,
    pub file_diff: Option<git::ChangedFile>,
    pub files: Option<Result<Vec<git::ChangedFile>, String>>,
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
    file_cache: HashMap<String, Vec<git::ChangedFile>>,
    requested_files_oid: Option<String>,
    pending_row_offset: Option<usize>,
    file_request_id: u64,
    requested_file: bool,
}

impl App {
    pub fn new(repo: Repository, limit: usize, demo: bool) -> Self {
        let graph = Graph::build(&repo.commits);
        let selected = (!repo.commits.is_empty()).then_some(0);
        let mut app = Self {
            theme: crate::theme::Theme::default(),
            repo,
            graph,
            selected,
            top: 0,
            top_offset: 0,
            history_rows: Vec::new(),
            history_file_rows: Vec::new(),
            file_selected: None,
            file_diff: None,
            files: None,
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
            file_cache: HashMap::new(),
            requested_files_oid: None,
            pending_row_offset: None,
            file_request_id: 0,
            requested_file: false,
        };
        app.selection_changed();
        app
    }

    pub fn with_sidebar(mut self, sidebar: bool) -> Self {
        self.sidebar = sidebar;
        self.show_details = !sidebar;
        self.focus = Focus::History;
        if sidebar {
            self.selected = None;
            self.selection_changed();
        }
        self
    }

    pub fn selected_oid(&self) -> Option<&str> {
        self.selected
            .and_then(|index| self.repo.commits.get(index))
            .map(|c| c.oid.as_str())
    }

    pub fn expanded_file_rows(&self) -> usize {
        if !self.sidebar || self.selected.is_none() {
            return 0;
        }
        self.files.as_ref().map_or(1, |result| {
            result.as_ref().map_or(1, |files| files.len().max(1))
        })
    }

    pub fn saved_row_offset(&self) -> usize {
        self.pending_row_offset.unwrap_or(self.top_offset)
    }

    pub fn restore_row_offset(&mut self, offset: usize) {
        self.pending_row_offset =
            (self.selected == Some(self.top) && self.files.is_none() && offset > 1)
                .then_some(offset);
        self.top_offset = if self.pending_row_offset.is_some() {
            0
        } else {
            offset
        };
    }

    pub fn history_row_height(&self, index: usize) -> usize {
        2 + if self.selected == Some(index) {
            self.expanded_file_rows()
        } else {
            0
        }
    }

    pub fn history_total_rows(&self) -> usize {
        self.repo.commits.len() * 2 + self.expanded_file_rows()
    }

    pub fn history_scroll_position(&self) -> usize {
        (self.top * 2)
            .saturating_add(self.top_offset)
            .saturating_add(if self.selected.is_some_and(|index| index < self.top) {
                self.expanded_file_rows()
            } else {
                0
            })
    }

    pub fn scroll_history_to(&mut self, position: usize) {
        let mut remaining = position.min(
            self.history_total_rows()
                .saturating_sub(usize::from(self.history_area.height)),
        );
        self.top = 0;
        while self.top < self.repo.commits.len() && remaining >= self.history_row_height(self.top) {
            remaining -= self.history_row_height(self.top);
            self.top += 1;
        }
        self.top_offset = remaining;
    }

    fn keep_selection_visible(&mut self) {
        if let Some(selected) = self.selected {
            if self.sidebar {
                let start = selected * 2;
                let position = self.history_scroll_position();
                let visible = usize::from(self.history_area.height).max(1);
                if start < position {
                    self.scroll_history_to(start);
                } else if start + 3 > position + visible {
                    self.scroll_history_to((start + 3).saturating_sub(visible));
                }
            } else {
                let count = usize::from(self.history_area.height / 2).max(1);
                if selected < self.top {
                    self.top = selected;
                } else if selected >= self.top + count {
                    self.top = selected + 1 - count;
                }
            }
        }
    }

    fn clear_sidebar_selection(&mut self) {
        if self.selected == Some(self.top) {
            self.top_offset = 0;
        }
        self.focus = Focus::History;
        self.selected = None;
        self.selection_changed();
    }

    fn toggle_sidebar_commit(&mut self, index: usize) {
        if self.selected == Some(index) {
            self.clear_sidebar_selection();
        } else {
            if self.selected == Some(self.top) {
                self.top_offset = 0;
            }
            self.selected = Some(index);
            self.selection_changed();
            self.keep_selection_visible();
        }
    }

    pub fn detail_line_count(&self) -> usize {
        self.detail.lines().count()
            + usize::from(
                self.file_diff.is_none()
                    && self
                        .selected
                        .and_then(|index| self.repo.commits.get(index))
                        .is_some_and(|commit| !commit.is_worktree() && !commit.refs.is_empty()),
            )
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
                detail: !self.sidebar
                    && self.show_details
                    && self.selected_oid() == Some(git::WORKTREE_OID),
            })
            .is_ok()
        {
            self.refreshing = true;
        }
    }

    pub fn selection_changed(&mut self) {
        self.file_selected = None;
        self.file_diff = None;
        self.file_request_id += 1;
        self.requested_file = false;
        if self.sidebar {
            self.show_details = false;
            self.focus = Focus::History;
        }
        self.detail_scroll = 0;
        self.detail_horizontal = 0;
        self.requested_oid = None;
        self.requested_files_oid = None;
        self.pending_row_offset = None;
        self.files = self.selected_oid().and_then(|oid| {
            if oid == git::WORKTREE_OID {
                Some(Ok(self.repo.worktree.files.clone()))
            } else if self.demo {
                Some(Ok(git::demo_changed_files()))
            } else {
                self.file_cache.get(oid).cloned().map(Ok)
            }
        });
        self.detail_due = Instant::now() + Duration::from_millis(100);
        self.detail = if self.demo {
            self.selected
                .and_then(|index| self.repo.commits.get(index))
                .map(git::demo_details)
                .unwrap_or_default()
        } else if let Some(oid) = self.selected_oid() {
            self.cache
                .get(oid)
                .cloned()
                .unwrap_or_else(|| "Loading commit…".into())
        } else {
            "No commits to inspect.".into()
        };
    }

    pub fn open_file(&mut self, index: usize) {
        let Some(file) = self
            .files
            .as_ref()
            .and_then(|result| result.as_ref().ok())
            .and_then(|files| files.get(index))
            .cloned()
        else {
            return;
        };
        self.file_selected = Some(index);
        self.detail = if self.demo {
            git::demo_file_details(&file.path)
        } else {
            "Loading file…".into()
        };
        self.file_diff = Some(file);
        self.focus = Focus::Details;
        self.show_details = true;
        self.detail_scroll = 0;
        self.detail_horizontal = 0;
        self.file_request_id += 1;
        self.requested_file = false;
        self.detail_due = Instant::now();
    }

    fn close_file(&mut self) {
        self.file_diff = None;
        self.show_details = false;
        self.focus = Focus::Files;
        self.file_request_id += 1;
        self.requested_file = false;
    }

    pub fn request_detail(&mut self, worker: &Worker) {
        if self.sidebar
            && let Some(file) = self.file_diff.clone()
        {
            if !self.demo
                && !self.loading
                && !self.requested_file
                && Instant::now() >= self.detail_due
                && let Some(oid) = self.selected_oid().map(str::to_owned)
            {
                self.requested_file = worker
                    .sender
                    .send(Request::FileDetail {
                        oid,
                        file,
                        id: self.file_request_id,
                    })
                    .is_ok();
            }
            return;
        }
        if self.sidebar {
            if !self.demo
                && !self.loading
                && self.files.is_none()
                && Instant::now() >= self.detail_due
                && let Some(oid) = self.selected_oid().map(str::to_owned)
                && self.requested_files_oid.as_ref() != Some(&oid)
            {
                let _ = worker.sender.send(Request::Files {
                    oid: oid.clone(),
                    id: self.load_id,
                });
                self.requested_files_oid = Some(oid);
            }
            return;
        }
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
                            .or_else(|| {
                                (!self.sidebar && !self.repo.commits.is_empty()).then_some(0)
                            });
                        if self.sidebar && self.selected.is_none() {
                            self.clear_sidebar_selection();
                        }
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
                        self.file_cache.clear();
                        self.top_offset = 0;
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
                    let top_oid = self.repo.commits.get(old_top).map(|c| c.oid.clone());
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
                        .or_else(|| {
                            (!self.sidebar && !self.repo.commits.is_empty())
                                .then(|| old_selected.unwrap_or(0).min(self.repo.commits.len() - 1))
                        });
                    self.top = if old_top == 0 {
                        0
                    } else {
                        top_oid
                            .and_then(|oid| self.repo.commits.iter().position(|c| c.oid == oid))
                            .unwrap_or(old_top.min(self.repo.commits.len().saturating_sub(1)))
                    };
                    if self.sidebar && self.selected.is_none() {
                        self.clear_sidebar_selection();
                    }
                    self.branch_selected = branch
                        .and_then(|r| self.repo.branches.iter().position(|b| b.reference == r))
                        .map(|i| i + 1)
                        .unwrap_or(0);
                    if self.selected_oid() != old.as_deref() {
                        self.selection_changed();
                    }
                }
                if self.selected_oid() == Some(git::WORKTREE_OID) {
                    if self.sidebar {
                        let selected_path = self.file_selected.and_then(|index| {
                            self.files
                                .as_ref()
                                .and_then(|result| result.as_ref().ok())
                                .and_then(|files| files.get(index))
                                .map(|file| file.path.clone())
                        });
                        self.files = Some(Ok(self.repo.worktree.files.clone()));
                        self.file_selected = selected_path.and_then(|path| {
                            self.repo
                                .worktree
                                .files
                                .iter()
                                .position(|file| file.path == path)
                        });
                        if let Some(file) = &self.file_diff {
                            if let Some(index) = self
                                .repo
                                .worktree
                                .files
                                .iter()
                                .position(|f| f.path == file.path)
                            {
                                self.file_selected = Some(index);
                                self.file_diff = Some(self.repo.worktree.files[index].clone());
                                self.file_request_id += 1;
                                self.requested_file = false;
                                self.detail_due = Instant::now();
                            } else {
                                self.close_file();
                                self.file_selected = None;
                                self.focus = Focus::History;
                            }
                        } else if self.focus == Focus::Files && self.file_selected.is_none() {
                            self.focus = Focus::History;
                        }
                    }
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
            Response::FileDetail {
                oid,
                file,
                id,
                result,
            } if id == self.file_request_id
                && self.selected_oid() == Some(&oid)
                && self.file_diff.as_ref() == Some(&file) =>
            {
                self.detail =
                    result.unwrap_or_else(|e| format!("Cannot load file: {e}\nPress r to retry."));
                self.detail_scroll = self
                    .detail_scroll
                    .min(self.detail_line_count().saturating_sub(1));
            }
            Response::Files { oid, id, result } if id == self.load_id => {
                if self.selected_oid() == Some(&oid) {
                    self.files = Some(result.clone());
                    if let Some(offset) = self.pending_row_offset.take() {
                        self.top = self.selected.unwrap();
                        self.top_offset = offset;
                    }
                }
                if let Ok(files) = result
                    && oid != git::WORKTREE_OID
                {
                    if self.file_cache.len() >= 64 {
                        self.file_cache.clear();
                    }
                    self.file_cache.insert(oid, files);
                }
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
                self.selected = if self.repo.commits.is_empty() {
                    None
                } else {
                    Some(
                        match self.selected {
                            Some(selected) => selected.saturating_add_signed(delta),
                            None if delta.abs() == 1 => self.top,
                            None => self.top.saturating_add_signed(delta),
                        }
                        .min(self.repo.commits.len() - 1),
                    )
                };
                self.selection_changed();
                self.keep_selection_visible();
            }
            Focus::Files => {
                if let Some(files) = self.files.as_ref().and_then(|result| result.as_ref().ok())
                    && !files.is_empty()
                {
                    let index = self
                        .file_selected
                        .unwrap_or(0)
                        .saturating_add_signed(delta)
                        .min(files.len() - 1);
                    self.file_selected = Some(index);
                    let start = self.selected.unwrap() * 2 + 2 + index;
                    let position = self.history_scroll_position();
                    let visible = usize::from(self.history_area.height).max(1);
                    if start < position {
                        self.scroll_history_to(start);
                    } else if start >= position + visible {
                        self.scroll_history_to(start + 1 - visible);
                    }
                }
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
                    .min(self.detail_line_count().saturating_sub(1))
            }
        }
    }

    pub fn find(&mut self, backwards: bool, include_current: bool) {
        let count = self.repo.commits.len();
        if count == 0 || self.query.is_empty() {
            return;
        }
        let query = self.query.to_lowercase();
        let start = usize::from(!include_current && self.selected.is_some());
        let selected = self.selected.unwrap_or(self.top).min(count - 1);
        for step in start..(count + start) {
            let index = if backwards {
                (selected + count - step % count) % count
            } else {
                (selected + step) % count
            };
            let c = &self.repo.commits[index];
            if [&c.oid, &c.author, &c.subject, &c.refs]
                .iter()
                .any(|s| s.to_lowercase().contains(&query))
            {
                self.selected = Some(index);
                self.selection_changed();
                self.keep_selection_visible();
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
            KeyCode::Esc if self.sidebar && self.file_diff.is_some() => self.close_file(),
            KeyCode::Char('r') if self.sidebar && self.file_diff.is_some() => {
                self.file_request_id += 1;
                self.requested_file = false;
                self.detail_due = Instant::now();
            }
            KeyCode::Tab | KeyCode::BackTab if self.sidebar => {
                if self.file_diff.is_none() {
                    if self.focus == Focus::Files {
                        self.focus = Focus::History;
                    } else if self
                        .files
                        .as_ref()
                        .and_then(|result| result.as_ref().ok())
                        .is_some_and(|files| !files.is_empty())
                    {
                        self.focus = Focus::Files;
                        self.file_selected = Some(0);
                        self.navigate(0);
                    }
                }
            }
            KeyCode::Char('d') if self.sidebar => {}
            KeyCode::Enter if self.sidebar && self.focus == Focus::Files => {
                if let Some(index) = self.file_selected {
                    self.open_file(index);
                }
            }
            KeyCode::Enter if self.sidebar && self.file_diff.is_some() => {}
            KeyCode::Enter if self.sidebar && !self.repo.commits.is_empty() => {
                self.toggle_sidebar_commit(self.selected.unwrap_or(self.top));
            }
            KeyCode::Enter if self.sidebar => {}
            KeyCode::Esc if self.sidebar && self.selected.is_some() => {
                self.clear_sidebar_selection()
            }
            KeyCode::Tab => {
                self.focus = match self.focus {
                    Focus::Branches => Focus::History,
                    Focus::History if self.show_details => Focus::Details,
                    _ => Focus::Branches,
                }
            }
            KeyCode::BackTab => {
                self.focus = match self.focus {
                    Focus::Details | Focus::Files => Focus::History,
                    Focus::History => Focus::Branches,
                    _ if self.show_details => Focus::Details,
                    _ => Focus::History,
                }
            }
            KeyCode::Char('/') => {
                if self.sidebar && self.file_diff.is_some() {
                    self.close_file();
                }
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
            KeyCode::PageDown if self.sidebar && self.focus != Focus::Details => {
                self.pending_row_offset = None;
                self.scroll_history_to(
                    self.history_scroll_position() + usize::from(self.history_area.height),
                );
            }
            KeyCode::PageUp if self.sidebar && self.focus != Focus::Details => {
                self.pending_row_offset = None;
                self.scroll_history_to(
                    self.history_scroll_position()
                        .saturating_sub(usize::from(self.history_area.height)),
                );
            }
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
            MouseEventKind::ScrollDown | MouseEventKind::ScrollUp
                if self.sidebar && self.focus == Focus::History =>
            {
                self.pending_row_offset = None;
                let step = usize::from(self.history_area.height).min(3) as isize;
                let delta = if event.kind == MouseEventKind::ScrollDown {
                    step
                } else {
                    -step
                };
                self.scroll_history_to(self.history_scroll_position().saturating_add_signed(delta));
            }
            MouseEventKind::ScrollDown => self.navigate(3),
            MouseEventKind::ScrollUp => self.navigate(-3),
            MouseEventKind::Down(crossterm::event::MouseButton::Left) => match self.focus {
                Focus::History => {
                    let row = usize::from(event.row.saturating_sub(self.history_area.y));
                    if self.sidebar {
                        if let Some(index) = self.history_file_rows.get(row).copied().flatten() {
                            self.open_file(index);
                        } else if let Some(index) = self.history_rows.get(row).copied().flatten() {
                            self.toggle_sidebar_commit(index);
                        }
                    } else {
                        let index = self.top + row / 2;
                        if index < self.repo.commits.len() {
                            self.selected = Some(index);
                            self.selection_changed();
                        }
                    }
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
    fn file_diff_requests_are_scoped_and_stale_replies_are_ignored() {
        let mut app = App::new(git::demo(), 2000, false).with_sidebar(true);
        app.selected = Some(0);
        app.selection_changed();
        let file = git::ChangedFile {
            status: "M".into(),
            path: "selected.rs".into(),
            original_path: None,
        };
        app.files = Some(Ok(vec![file.clone()]));
        let (sender, requests) = mpsc::channel();
        let (_, receiver) = mpsc::channel();
        let worker = Worker { sender, receiver };
        app.open_file(0);
        app.request_detail(&worker);
        let Request::FileDetail {
            oid,
            file: requested,
            id,
        } = requests.try_recv().unwrap()
        else {
            panic!("Expected a file-scoped request");
        };
        assert_eq!(requested, file);
        app.request_detail(&worker);
        assert!(requests.try_recv().is_err());
        app.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), None);
        assert_eq!(app.focus, Focus::Files);
        assert!(app.file_diff.is_none());
        app.open_file(0);
        assert!(!app.apply(Response::FileDetail {
            oid: oid.clone(),
            file: file.clone(),
            id,
            result: Ok("stale diff".into())
        }));
        app.request_detail(&worker);
        let Request::FileDetail { id, .. } = requests.try_recv().unwrap() else {
            panic!("Expected a file-scoped request");
        };
        assert!(app.apply(Response::FileDetail {
            oid,
            file,
            id,
            result: Err("failed".into())
        }));
        assert!(app.detail.contains("Press r to retry"));
        app.key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE), None);
        app.request_detail(&worker);
        assert!(matches!(
            requests.try_recv().unwrap(),
            Request::FileDetail { .. }
        ));
        app.selection_changed();
        assert!(app.file_diff.is_none());
        assert_eq!(app.focus, Focus::History);
    }

    #[test]
    fn worktree_refresh_preserves_the_keyboard_selected_file_by_path() {
        let file = |path: &str| git::ChangedFile {
            status: " M".into(),
            path: path.into(),
            original_path: None,
        };
        let mut repo = with_worktree(git::demo());
        repo.worktree.files = vec![file("b.rs"), file("c.rs")];
        let mut app = App::new(repo, 2000, false).with_sidebar(true);
        app.selected = Some(0);
        app.selection_changed();
        app.focus = Focus::Files;
        app.file_selected = Some(1);
        let mut repo = app.repo.clone();
        repo.worktree.files.insert(0, file("a.rs"));
        assert!(app.apply(refreshed(repo, None)));
        assert_eq!(app.file_selected, Some(2));
        assert_eq!(app.focus, Focus::Files);
        app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), None);
        assert_eq!(app.file_diff.as_ref().unwrap().path, "c.rs");
        app.close_file();
        let mut repo = app.repo.clone();
        repo.worktree.files.pop();
        assert!(app.apply(refreshed(repo, None)));
        assert_eq!(app.file_selected, None);
        assert_eq!(app.focus, Focus::History);
    }

    #[test]
    fn worktree_file_diff_refreshes_even_when_status_is_unchanged() {
        let mut repo = with_worktree(git::demo());
        let file = git::ChangedFile {
            status: " M".into(),
            path: "tracked.rs".into(),
            original_path: None,
        };
        repo.worktree.files = vec![file.clone()];
        let mut app = App::new(repo, 2000, false).with_sidebar(true);
        app.selected = Some(0);
        app.selection_changed();
        app.open_file(0);
        app.detail = "old contents".into();
        app.requested_file = true;
        let old_request = app.file_request_id;
        assert!(!app.apply(refreshed(app.repo.clone(), None)));
        assert!(!app.requested_file);
        assert!(app.file_request_id > old_request);
        assert_eq!(app.detail, "old contents");
        assert!(!app.apply(Response::FileDetail {
            oid: git::WORKTREE_OID.into(),
            file,
            id: old_request,
            result: Ok("stale".into())
        }));
        let mut repo = app.repo.clone();
        repo.worktree.files.clear();
        assert!(app.apply(refreshed(repo, None)));
        assert!(app.file_diff.is_none());
        assert_eq!(app.focus, Focus::History);
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
        assert_eq!(app.selected, Some(1));
        assert_eq!(app.top, 0);
        assert_eq!(app.graph_offset, 3);
        assert_eq!(app.detail_scroll, 4);
        assert_eq!(app.detail, "existing commit diff");

        app.selected = Some(7);
        app.top = 5;
        let oid = app.selected_oid().unwrap().to_owned();
        assert!(app.apply(refreshed(git::demo(), None)));
        assert_eq!(app.selected_oid(), Some(oid.as_str()));
        assert_eq!(app.top, 4);
        assert_eq!(app.selected.unwrap() - app.top, 2);
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
        assert_eq!(app.selected, Some(0));
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
        for code in [KeyCode::Tab, KeyCode::BackTab, KeyCode::Char('d')] {
            app.key(KeyEvent::new(code, KeyModifiers::NONE), None);
            assert_eq!(app.focus, Focus::History);
            assert!(!app.show_details);
        }
        app.key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE), None);
        assert_eq!(app.selected, Some(0));
        let (sender, requests) = mpsc::channel();
        let (_, receiver) = mpsc::channel();
        let worker = Worker { sender, receiver };
        app.detail_due = Instant::now();
        app.request_detail(&worker);
        assert!(matches!(requests.try_recv(), Ok(Request::Files { .. })));
        app.request_detail(&worker);
        assert!(requests.try_recv().is_err());
        app.query = "한글".into();
        app.find(false, true);
        assert_eq!(app.selected, Some(8));
        app.load(None, Some(&worker));
        assert!(matches!(requests.try_recv(), Ok(Request::Load { .. })));
    }

    fn sidebar_mouse(app: &mut App, kind: MouseEventKind, row: u16) {
        app.history_rows = (0..usize::from(app.history_area.height))
            .map(|line| {
                let index = app.top + line / 2;
                (index < app.repo.commits.len()).then_some(index)
            })
            .collect();
        app.mouse(
            MouseEvent {
                kind,
                column: app.history_area.x + 1,
                row,
                modifiers: KeyModifiers::NONE,
            },
            None,
        );
    }

    #[test]
    fn sidebar_wheel_scrolls_the_viewport_without_selecting_or_snapping_back() {
        let mut app = App::new(git::demo(), 2000, false).with_sidebar(true);
        app.history_area = Rect::new(1, 3, 30, 6);
        assert_eq!(app.selected, None);
        assert_eq!(app.selected_oid(), None);
        sidebar_mouse(&mut app, MouseEventKind::ScrollDown, 3);
        assert_eq!(app.history_scroll_position(), 3);
        assert_eq!(app.selected, None);
        app.selected = Some(1);
        sidebar_mouse(&mut app, MouseEventKind::ScrollDown, 3);
        assert_eq!(app.history_scroll_position(), 6);
        assert_eq!(app.selected, Some(1));
        sidebar_mouse(&mut app, MouseEventKind::ScrollDown, 3);
        assert_eq!(app.history_scroll_position(), 9);
        sidebar_mouse(&mut app, MouseEventKind::ScrollUp, 3);
        assert_eq!(app.history_scroll_position(), 6);
        for _ in 0..3 {
            sidebar_mouse(&mut app, MouseEventKind::ScrollUp, 3);
        }
        assert_eq!(app.top, 0);
        assert!(!app.show_details);
    }

    #[test]
    fn sidebar_click_expands_files_and_escape_collapses_without_replacing_history() {
        let mut app = App::new(git::demo(), 2000, false).with_sidebar(true);
        app.history_area = Rect::new(1, 3, 30, 6);
        app.top = 4;
        sidebar_mouse(
            &mut app,
            MouseEventKind::Down(crossterm::event::MouseButton::Left),
            5,
        );
        assert_eq!(app.selected, Some(5));
        assert_eq!(app.focus, Focus::History);
        assert!(!app.show_details);
        let (sender, requests) = mpsc::channel();
        let (_, receiver) = mpsc::channel();
        let worker = Worker { sender, receiver };
        app.detail_due = Instant::now();
        app.request_detail(&worker);
        assert!(
            matches!(requests.try_recv(), Ok(Request::Files { oid, .. }) if oid == app.repo.commits[5].oid)
        );
        app.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), None);
        assert_eq!(app.selected, None);
        assert_eq!(app.focus, Focus::History);
        assert!(!app.show_details);
        assert_eq!(app.top, 4);
        app.request_detail(&worker);
        assert!(requests.try_recv().is_err());
    }

    #[test]
    fn sidebar_keyboard_can_select_open_and_leave_a_commit() {
        let mut app = App::new(git::demo(), 2000, true).with_sidebar(true);
        app.history_area = Rect::new(1, 3, 30, 6);
        app.key(KeyEvent::new(KeyCode::End, KeyModifiers::NONE), None);
        assert_eq!(app.selected, Some(9));
        assert_eq!(app.top, 7);
        assert!(!app.show_details);
        assert_eq!(
            app.files.as_ref().unwrap().as_ref().unwrap()[0].path,
            "src/curves.rs"
        );
        app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), None);
        assert_eq!(app.selected, None);
        app.key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE), None);
        assert_eq!(app.selected, Some(7));
        app.key(KeyEvent::new(KeyCode::Home, KeyModifiers::NONE), None);
        assert_eq!(app.selected, Some(0));
        assert_eq!(app.top, 0);
    }

    #[test]
    fn demo_files_show_added_modified_deleted_and_open_only_the_selected_patch() {
        let mut app = App::new(git::demo(), 2000, true).with_sidebar(true);
        app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), None);
        let files = app.files.as_ref().unwrap().as_ref().unwrap().clone();
        assert_eq!(
            files
                .iter()
                .map(|file| (file.status.as_str(), file.path.as_str()))
                .collect::<Vec<_>>(),
            [
                ("A", "src/curves.rs"),
                ("M", "src/graph.rs"),
                ("D", "src/legacy_graph.rs"),
            ]
        );
        let commit_detail = app.detail.clone();
        for (index, file) in files.iter().enumerate() {
            app.open_file(index);
            assert_eq!(app.file_diff.as_ref(), Some(file));
            assert_eq!(app.focus, Focus::Details);
            assert!(app.show_details);
            assert!(
                app.detail
                    .contains(&format!("diff --git a/{0} b/{0}", file.path))
            );
            for other in files.iter().filter(|other| other.path != file.path) {
                assert!(!app.detail.contains(&other.path));
            }
            match file.status.as_str() {
                "A" => assert!(app.detail.contains("new file mode 100644\n--- /dev/null")),
                "M" => assert!(
                    app.detail.contains("-    draw_ascii_edge")
                        && app.detail.contains("+    let curve")
                ),
                "D" => assert!(
                    app.detail.contains("deleted file mode 100644")
                        && app.detail.contains("+++ /dev/null")
                ),
                _ => unreachable!(),
            }
            assert!(commit_detail.contains(app.detail.split("\n[Demo data").next().unwrap()));
            app.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), None);
            assert!(app.file_diff.is_none());
            assert_eq!(app.selected, Some(0));
            assert_eq!(app.files.as_ref().unwrap().as_ref().unwrap(), &files);
        }
    }

    #[test]
    fn sidebar_refresh_keeps_an_unselected_viewport_anchored_to_its_commit() {
        let mut app = App::new(git::demo(), 2000, false).with_sidebar(true);
        app.top = 4;
        let top_oid = app.repo.commits[4].oid.clone();
        assert!(app.apply(refreshed(with_worktree(app.repo.clone()), None)));
        assert_eq!(app.selected, None);
        assert_eq!(app.top, 5);
        assert_eq!(app.repo.commits[app.top].oid, top_oid);
        assert!(!app.show_details);
        let mut repo = app.repo.clone();
        repo.commits.clear();
        assert!(app.apply(refreshed(repo, None)));
        assert_eq!(app.selected, None);
        assert_eq!(app.top, 0);
    }

    #[test]
    fn sidebar_does_not_open_blank_rows_or_switch_to_an_unrelated_commit_after_refresh() {
        let mut app = App::new(git::demo(), 2000, false).with_sidebar(true);
        app.history_area = Rect::new(1, 3, 30, 30);
        sidebar_mouse(
            &mut app,
            MouseEventKind::Down(crossterm::event::MouseButton::Left),
            25,
        );
        assert_eq!(app.selected, None);
        assert!(!app.show_details);
        sidebar_mouse(
            &mut app,
            MouseEventKind::Down(crossterm::event::MouseButton::Left),
            5,
        );
        assert_eq!(app.selected, Some(1));
        let mut repo = app.repo.clone();
        repo.commits.remove(1);
        assert!(app.apply(refreshed(repo, None)));
        assert_eq!(app.selected, None);
        assert!(!app.show_details);
        assert_eq!(app.focus, Focus::History);
    }

    #[test]
    fn late_or_stale_file_responses_do_not_replace_another_commits_files() {
        let mut app = App::new(git::demo(), 2000, false).with_sidebar(true);
        app.toggle_sidebar_commit(0);
        let first = app.selected_oid().unwrap().to_owned();
        app.toggle_sidebar_commit(1);
        let second = app.selected_oid().unwrap().to_owned();
        let files = vec![git::ChangedFile {
            status: "M".into(),
            path: "first.rs".into(),
            original_path: None,
        }];
        app.apply(Response::Files {
            oid: first.clone(),
            id: 0,
            result: Ok(files.clone()),
        });
        assert!(app.files.is_none());
        assert!(!app.apply(Response::Files {
            oid: second.clone(),
            id: 1,
            result: Ok(files.clone())
        }));
        assert!(app.files.is_none());
        app.apply(Response::Files {
            oid: second,
            id: 0,
            result: Err("read failed".into()),
        });
        assert!(app.files.as_ref().unwrap().is_err());
        app.toggle_sidebar_commit(0);
        assert_eq!(app.files, Some(Ok(files)));
        assert_eq!(app.selected_oid(), Some(first.as_str()));
    }

    #[test]
    fn sidebar_worktree_files_update_without_loading_a_patch() {
        let mut app = App::new(with_worktree(git::demo()), 2000, false).with_sidebar(true);
        app.toggle_sidebar_commit(0);
        assert_eq!(app.files, Some(Ok(Vec::new())));
        let (sender, requests) = mpsc::channel();
        let (_, receiver) = mpsc::channel();
        let worker = Worker { sender, receiver };
        app.detail_due = Instant::now();
        app.request_detail(&worker);
        assert!(requests.try_recv().is_err());
        let mut repo = app.repo.clone();
        let file = git::ChangedFile {
            status: "??".into(),
            path: "new.txt".into(),
            original_path: None,
        };
        repo.worktree.files.push(file.clone());
        repo.worktree.untracked = 1;
        assert!(app.apply(refreshed(repo, None)));
        assert_eq!(app.files, Some(Ok(vec![file])));
        assert!(!app.show_details);
    }

    #[test]
    fn unicode_search_keeps_graph_topology_and_wraps() {
        let mut app = App::new(git::demo(), 2000, true);
        app.query = "한글".into();
        app.find(false, true);
        assert_eq!(app.selected, Some(8));
        assert_eq!(app.graph.rows.len(), 10);
        app.find(false, false);
        assert_eq!(app.selected, Some(8));
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
