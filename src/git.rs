use anyhow::{Context, Result, bail};
use std::{
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

pub const WORKTREE_OID: &str = "WORKTREE";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Commit {
    pub oid: String,
    pub parents: Vec<String>,
    pub author: String,
    pub date: String,
    pub refs: String,
    pub subject: String,
}

impl Commit {
    pub fn is_worktree(&self) -> bool {
        self.oid == WORKTREE_OID
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Branch {
    pub reference: String,
    pub name: String,
    pub oid: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WorkingTree {
    pub files: Vec<ChangedFile>,
    pub staged: usize,
    pub unstaged: usize,
    pub untracked: usize,
    pub conflicts: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChangedFile {
    pub status: String,
    pub path: String,
    pub original_path: Option<String>,
}

impl WorkingTree {
    pub fn summary(&self) -> String {
        [
            (self.staged, "staged"),
            (self.unstaged, "unstaged"),
            (self.untracked, "untracked"),
            (self.conflicts, "conflicts"),
        ]
        .into_iter()
        .filter(|(count, _)| *count > 0)
        .map(|(count, label)| format!("{count} {label}"))
        .collect::<Vec<_>>()
        .join(" · ")
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Repository {
    pub root: PathBuf,
    pub head: String,
    pub commits: Vec<Commit>,
    pub branches: Vec<Branch>,
    pub worktree: WorkingTree,
    pub head_oid: Option<String>,
    refs_snapshot: String,
    loaded_reference: Option<String>,
    loaded_limit: usize,
}

impl Repository {
    pub fn commit_count(&self) -> usize {
        self.commits.iter().filter(|c| !c.is_worktree()).count()
    }
}

// Subprocesses are bounded and never invoke a shell, a pager, or external diff tools.
fn run(path: &Path, args: &[&str], limit: usize) -> Result<String> {
    let mut child = Command::new("git")
        .args(["--no-pager", "--no-optional-locks", "-c", "color.ui=false"])
        .arg("-C")
        .arg(path)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("Git을 실행할 수 없습니다. Git 설치를 확인하세요.")?;
    let stdout = child.stdout.take().context("Git stdout unavailable")?;
    let stderr = child.stderr.take().context("Git stderr unavailable")?;
    let out = thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout
            .take(limit as u64 + 1)
            .read_to_end(&mut bytes)
            .map(|_| bytes)
    });
    let err = thread::spawn(move || {
        let mut bytes = Vec::new();
        stderr.take(65536).read_to_end(&mut bytes).map(|_| bytes)
    });
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if start.elapsed() > Duration::from_secs(15) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = out.join();
            let _ = err.join();
            bail!("Git 조회가 15초를 초과했습니다.");
        }
        thread::sleep(Duration::from_millis(10));
    };
    let mut bytes = out
        .join()
        .map_err(|_| anyhow::anyhow!("Git reader failed"))??;
    let error = err
        .join()
        .map_err(|_| anyhow::anyhow!("Git reader failed"))??;
    if bytes.len() > limit {
        bytes.truncate(limit);
        if matches!(args.first(), Some(&"show" | &"diff")) {
            return Ok(format!(
                "{}\n\n[Diff preview truncated at 512 KiB]",
                String::from_utf8_lossy(&bytes)
            ));
        }
        bail!("Git 응답이 너무 큽니다. --limit 값을 줄이세요.");
    }
    if !status.success() {
        bail!("{}", String::from_utf8_lossy(&error).trim());
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

pub fn discover(path: &Path) -> Result<PathBuf> {
    let root = run(path, &["rev-parse", "--show-toplevel"], 65536)
        .with_context(|| format!("Git 작업 폴더가 아닙니다: {}", path.display()))?;
    Ok(PathBuf::from(root.trim_end_matches('\n')))
}

pub fn load(root: &Path, reference: Option<&str>, limit: usize) -> Result<Repository> {
    read_repository(root, reference, limit, None)
}

/// Reuse the loaded history while refs and HEAD are unchanged.
pub fn refresh(previous: &Repository, reference: Option<&str>, limit: usize) -> Result<Repository> {
    read_repository(&previous.root, reference, limit, Some(previous))
}

fn read_repository(
    root: &Path,
    reference: Option<&str>,
    limit: usize,
    previous: Option<&Repository>,
) -> Result<Repository> {
    if reference.is_some_and(|r| r.starts_with('-')) {
        bail!("잘못된 Git ref입니다.");
    }
    let head_reference = run(root, &["symbolic-ref", "--quiet", "HEAD"], 65536)
        .ok()
        .map(|s| s.trim().to_owned());
    let head = head_reference
        .as_deref()
        .map(|r| r.strip_prefix("refs/heads/").unwrap_or(r))
        .unwrap_or("detached HEAD")
        .to_owned();
    let raw_refs = run(
        root,
        &[
            "for-each-ref",
            "--sort=refname",
            "--format=%(refname)%00%(refname:short)%00%(objectname)%00%(symref)",
        ],
        4 * 1024 * 1024,
    )?;
    let branches = raw_refs
        .lines()
        .filter_map(|line| {
            let parts: Vec<_> = line.split('\0').collect();
            (parts.len() == 4
                && parts[3].is_empty()
                && (parts[0].starts_with("refs/heads/") || parts[0].starts_with("refs/remotes/")))
            .then(|| Branch {
                reference: parts[0].into(),
                name: parts[1].into(),
                oid: parts[2].into(),
            })
        })
        .collect();
    let limit_arg = format!("--max-count={limit}");
    let mut args = vec![
        "log",
        "-z",
        "--topo-order",
        "--date=short",
        "--decorate=short",
        &limit_arg,
        "--format=%H%x00%P%x00%an%x00%ad%x00%D%x00%s",
    ];
    let head_oid = run(root, &["rev-parse", "--verify", "HEAD^{commit}"], 65536)
        .ok()
        .map(|s| s.trim().to_owned());
    if let Some(reference) = reference {
        args.push(reference);
    } else {
        args.push("--all");
        if head_oid.is_some() {
            args.push("HEAD");
        }
    }
    args.push("--");
    let mut commits = if let Some(previous) = previous.filter(|p| {
        p.head_oid == head_oid
            && p.head == head
            && p.refs_snapshot == raw_refs
            && p.loaded_reference.as_deref() == reference
            && p.loaded_limit == limit
    }) {
        previous
            .commits
            .iter()
            .filter(|c| !c.is_worktree())
            .cloned()
            .collect()
    } else if head_oid.is_none() && raw_refs.is_empty() && reference.is_none() {
        Vec::new()
    } else {
        parse_log(&run(root, &args, 32 * 1024 * 1024)?)?
    };
    let worktree = working_tree(root)?;
    let show_worktree = reference.is_none()
        || reference == Some("HEAD")
        || reference == head_reference.as_deref()
        || (head_reference.is_some() && reference == Some(head.as_str()));
    if show_worktree && !worktree.files.is_empty() {
        commits.insert(
            0,
            Commit {
                oid: WORKTREE_OID.into(),
                parents: head_oid.iter().cloned().collect(),
                author: "Working tree".into(),
                date: String::new(),
                refs: worktree.summary(),
                subject: "Uncommitted changes".into(),
            },
        );
    }
    Ok(Repository {
        root: root.into(),
        head,
        commits,
        branches,
        worktree,
        head_oid,
        refs_snapshot: raw_refs,
        loaded_reference: reference.map(str::to_owned),
        loaded_limit: limit,
    })
}

pub fn working_tree(root: &Path) -> Result<WorkingTree> {
    let raw = run(
        root,
        &[
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
            "--ignore-submodules=none",
        ],
        8 * 1024 * 1024,
    )?;
    parse_status(&raw)
}

fn parse_status(raw: &str) -> Result<WorkingTree> {
    let mut tree = WorkingTree::default();
    let mut records = raw.split_terminator('\0');
    while let Some(record) = records.next() {
        let bytes = record.as_bytes();
        if bytes.len() < 4 || bytes[2] != b' ' || !bytes[..2].is_ascii() {
            bail!("Git 상태 레코드 형식이 올바르지 않습니다.");
        }
        let status = &record[..2];
        let original_path = if bytes[..2].iter().any(|b| matches!(b, b'R' | b'C')) {
            Some(
                records
                    .next()
                    .context("Git rename path missing")?
                    .to_owned(),
            )
        } else {
            None
        };
        if status == "??" {
            tree.untracked += 1;
        } else if matches!(status, "DD" | "AU" | "UD" | "UA" | "DU" | "AA" | "UU") {
            tree.conflicts += 1;
        } else {
            tree.staged += usize::from(bytes[0] != b' ');
            tree.unstaged += usize::from(bytes[1] != b' ');
        }
        tree.files.push(ChangedFile {
            status: status.into(),
            path: record[3..].into(),
            original_path,
        });
    }
    Ok(tree)
}

pub fn parse_log(raw: &str) -> Result<Vec<Commit>> {
    if raw.is_empty() {
        return Ok(Vec::new());
    }
    // Only one terminating NUL is framing; subjects can be empty.
    let fields: Vec<_> = raw.strip_suffix('\0').unwrap_or(raw).split('\0').collect();
    if fields.len() % 6 != 0 {
        bail!("Git 로그 레코드 형식이 올바르지 않습니다.");
    }
    Ok(fields
        .as_chunks::<6>()
        .0
        .iter()
        .map(|f| Commit {
            oid: f[0].into(),
            parents: f[1].split_whitespace().map(str::to_owned).collect(),
            author: f[2].into(),
            date: f[3].into(),
            refs: f[4].into(),
            subject: f[5].into(),
        })
        .collect())
}

pub fn details(root: &Path, oid: &str) -> Result<String> {
    if oid == WORKTREE_OID {
        return working_tree_details(root, &working_tree(root)?);
    }
    if !oid.bytes().all(|b| b.is_ascii_hexdigit()) || oid.is_empty() {
        bail!("Invalid commit id");
    }
    run(
        root,
        &[
            "show",
            "--no-ext-diff",
            "--no-textconv",
            "--no-renames",
            "--format=fuller",
            "--stat",
            "--patch",
            "--diff-merges=first-parent",
            oid,
            "--",
        ],
        512 * 1024,
    )
}

pub fn working_tree_details(root: &Path, tree: &WorkingTree) -> Result<String> {
    let mut text = format!(
        "Uncommitted changes\n{}\n\nFiles (index / working tree):\n",
        tree.summary()
    );
    for file in &tree.files {
        let path = if let Some(original) = &file.original_path {
            format!("{} -> {}", display_path(original), display_path(&file.path))
        } else {
            display_path(&file.path)
        };
        text.push_str(&format!("{} {path}\n", file.status));
    }
    for (title, cached) in [("Staged changes", true), ("Unstaged changes", false)] {
        let mut args = vec![
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--no-color",
            "--no-renames",
            "--ignore-submodules=none",
            "--stat",
            "--patch",
        ];
        if cached {
            args.push("--cached");
        }
        args.push("--");
        let diff = run(root, &args, 512 * 1024)?;
        text.push_str(&format!(
            "\n{title}\n{}",
            if diff.is_empty() { "(none)\n" } else { &diff }
        ));
    }
    if tree.untracked > 0 {
        text.push_str("\nUntracked files are listed above as ??; their contents are not included in the diff.\n");
    }
    Ok(text)
}

fn display_path(path: &str) -> String {
    if path.chars().any(char::is_control) {
        format!("{path:?}")
    } else {
        path.to_owned()
    }
}

pub fn demo() -> Repository {
    let data = [
        (
            "a31c9f0",
            "b62d1e0 c83e2d0",
            "HEAD -> main, origin/main",
            "Merge pull request #42: curved graph renderer",
            "Mina Park",
        ),
        (
            "c83e2d0",
            "f16b5a0",
            "feature/smooth-curves",
            "Render antialiased Bézier branch connections",
            "Jun Lee",
        ),
        (
            "b62d1e0",
            "d94f3c0",
            "",
            "Add keyboard navigation and commit search",
            "Mina Park",
        ),
        (
            "d94f3c0",
            "f16b5a0 e05a4b0",
            "",
            "Merge branch 'feature/inspector'",
            "Mina Park",
        ),
        (
            "e05a4b0",
            "a27c6b0",
            "feature/inspector",
            "Show changed files and syntax-colored diffs",
            "Alex Kim",
        ),
        (
            "f16b5a0",
            "a27c6b0",
            "",
            "Keep graph lanes stable while scrolling",
            "Jun Lee",
        ),
        (
            "a27c6b0",
            "b38d7c0",
            "tag: v0.1.0",
            "Connect the graph to a Herdr workspace",
            "Alex Kim",
        ),
        (
            "b38d7c0",
            "c49e8d0",
            "",
            "Add local and remote branch navigation",
            "Jun Lee",
        ),
        (
            "c49e8d0",
            "d50f9e0",
            "",
            "한글 커밋 메시지와 검색 지원",
            "Jun Lee",
        ),
        ("d50f9e0", "", "", "Initial commit", "Mina Park"),
    ];
    Repository {
        root: PathBuf::from("herdr-git-graph"),
        head: "main".into(),
        worktree: WorkingTree::default(),
        head_oid: Some("a31c9f0".into()),
        refs_snapshot: String::new(),
        loaded_reference: None,
        loaded_limit: 2000,
        branches: vec![
            ("main", "a31c9f0"),
            ("feature/smooth-curves", "c83e2d0"),
            ("feature/inspector", "e05a4b0"),
            ("origin/main", "a31c9f0"),
        ]
        .into_iter()
        .map(|(name, oid)| Branch {
            reference: format!("refs/heads/{name}"),
            name: name.into(),
            oid: oid.into(),
        })
        .collect(),
        commits: data
            .into_iter()
            .enumerate()
            .map(|(i, (oid, parents, refs, subject, author))| Commit {
                oid: oid.into(),
                parents: parents.split_whitespace().map(str::to_owned).collect(),
                author: author.into(),
                date: format!("2026-09-{:02}", 12 - i.min(11)),
                refs: refs.into(),
                subject: subject.into(),
            })
            .collect(),
    }
}
