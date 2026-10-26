use anyhow::{Context, Result, bail};
use std::{
    io::{BufRead, BufReader, Read},
    path::{Component, Path, PathBuf},
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
    run_with_paths(path, args, limit, false)
}

fn run_with_paths(path: &Path, args: &[&str], limit: usize, literal: bool) -> Result<String> {
    run_filtered(path, args, limit, literal, None)
}

fn run_filtered(
    path: &Path,
    args: &[&str],
    limit: usize,
    literal: bool,
    selected: Option<ChangedFile>,
) -> Result<String> {
    let mut command = Command::new("git");
    command.args(["--no-pager", "--no-optional-locks", "-c", "color.ui=false"]);
    if literal {
        // Override inherited pathspec modes as well as disabling magic and globbing.
        command
            .arg("--literal-pathspecs")
            .env_remove("GIT_LITERAL_PATHSPECS")
            .env_remove("GIT_GLOB_PATHSPECS")
            .env_remove("GIT_NOGLOB_PATHSPECS")
            .env_remove("GIT_ICASE_PATHSPECS");
    }
    if selected.is_some() {
        command.args(["-c", "core.quotePath=true"]);
    }
    let mut child = command
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
        if let Some(file) = selected {
            return read_selected_patch(stdout, limit, &file);
        }
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

/// List changed paths without loading a patch. Merge commits use their first parent.
pub fn changed_files(root: &Path, oid: &str) -> Result<Vec<ChangedFile>> {
    if oid == WORKTREE_OID {
        return Ok(working_tree(root)?.files);
    }
    if !oid.bytes().all(|b| b.is_ascii_hexdigit()) || oid.is_empty() {
        bail!("Invalid commit id");
    }
    let ancestry = run(
        root,
        &["rev-list", "--parents", "-n", "1", oid, "--"],
        65536,
    )?;
    let mut revisions = ancestry.split_whitespace();
    let commit = revisions.next().context("Git commit missing")?;
    let parent = revisions.next();
    let mut args = vec![
        "diff-tree",
        "--root",
        "--no-commit-id",
        "--name-status",
        "-r",
        "-z",
        "--no-ext-diff",
        "--no-textconv",
        "--find-renames",
    ];
    if let Some(parent) = parent {
        args.push(parent);
    }
    args.extend([commit, "--"]);
    // Unlike patch previews, an oversized file list must fail, not return a partial list.
    parse_changed_files(&run(root, &args, 8 * 1024 * 1024)?)
}

fn parse_changed_files(raw: &str) -> Result<Vec<ChangedFile>> {
    if !raw.is_empty() && !raw.ends_with('\0') {
        bail!("Git changed-file record is incomplete");
    }
    let mut files = Vec::new();
    let mut records = raw.split_terminator('\0');
    while let Some(status) = records.next() {
        let bytes = status.as_bytes();
        if bytes.is_empty()
            || !matches!(
                bytes[0],
                b'A' | b'C' | b'D' | b'M' | b'R' | b'T' | b'U' | b'X' | b'B'
            )
            || !bytes[1..].iter().all(u8::is_ascii_digit)
        {
            bail!("Invalid Git changed-file status");
        }
        let first_path = records
            .next()
            .filter(|path| !path.is_empty())
            .context("Git changed-file path missing")?;
        let (path, original_path) = if matches!(bytes[0], b'R' | b'C') {
            let path = records
                .next()
                .filter(|path| !path.is_empty())
                .context("Git rename/copy destination missing")?;
            (path, Some(first_path.to_owned()))
        } else {
            (first_path, None)
        };
        files.push(ChangedFile {
            status: status.to_owned(),
            path: path.to_owned(),
            original_path,
        });
    }
    Ok(files)
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

const FILE_PREVIEW_LIMIT: usize = 512 * 1024;
const PREVIEW_TRUNCATED: &str = "\n\n[Diff preview truncated at 512 KiB]";

/// Preview only the selected path (and its rename source), without modifying the index.
/// Commit patches use the first parent; untracked contents are read in a bounded preview.
pub fn file_details(root: &Path, oid: &str, file: &ChangedFile) -> Result<String> {
    let mut paths = vec![file.path.as_str()];
    if let Some(original) = file.original_path.as_deref() {
        paths.push(original);
    }
    for path in &paths {
        validate_file_path(path)?;
    }
    if oid == WORKTREE_OID {
        // Do not follow symlinked parent directories, even for stale status records.
        let root = root.canonicalize().context("Git root unavailable")?;
        for path in &paths {
            validate_worktree_path(&root, path)?;
        }
        if file.status == "??" {
            return untracked_file_details(&root, &file.path);
        }
        let mut text = String::new();
        for (title, cached) in [("Staged changes", true), ("Unstaged changes", false)] {
            let mut args = vec![
                "diff",
                "--no-ext-diff",
                "--no-textconv",
                "--no-color",
                "--no-relative",
                "--find-renames",
                "--ignore-submodules=none",
                "--submodule=short",
                "--src-prefix=a/",
                "--dst-prefix=b/",
                "--line-prefix=",
                "--patch",
            ];
            if cached {
                args.push("--cached");
            }
            args.push("--");
            args.extend(paths.iter().copied());
            let patch = run_filtered(&root, &args, FILE_PREVIEW_LIMIT, true, Some(file.clone()))?;
            text.push_str(&format!(
                "{title}\n{}\n",
                if patch.is_empty() { "(none)\n" } else { &patch }
            ));
        }
        return Ok(bound_file_preview(text));
    }
    if oid.is_empty() || !oid.bytes().all(|b| b.is_ascii_hexdigit()) {
        bail!("Invalid commit id");
    }
    // A hexadecimal blob/tree id must not make `show` print non-patch contents.
    let revision = format!("{oid}^{{commit}}");
    let commit = run(root, &["rev-parse", "--verify", &revision], 65536)?;
    let mut args = vec![
        "show",
        "--format=",
        "--no-notes",
        "--no-ext-diff",
        "--no-textconv",
        "--no-color",
        "--no-relative",
        "--find-renames",
        "--ignore-submodules=none",
        "--submodule=short",
        "--src-prefix=a/",
        "--dst-prefix=b/",
        "--line-prefix=",
        "--patch",
        "--diff-merges=first-parent",
        commit.trim(),
        "--",
    ];
    args.extend(paths);
    run_filtered(root, &args, FILE_PREVIEW_LIMIT, true, Some(file.clone())).map(bound_file_preview)
}

// Literal pathspecs still select descendants and independent changes at a rename
// source. Filter complete patch records by exact endpoints, before counting bytes.
// Rename/copy metadata is authoritative: a diff header alone can be ambiguous when
// unquoted filenames contain spaces (including " b/").
fn read_selected_patch(
    reader: impl Read,
    limit: usize,
    file: &ChangedFile,
) -> std::io::Result<Vec<u8>> {
    #[derive(Default)]
    struct Record {
        pending: Vec<u8>,
        metadata: bool,
        ordinary: bool,
        renamed: bool,
        source: bool,
        destination: bool,
        selected: bool,
    }
    impl Record {
        fn finish_metadata(&mut self, output: &mut Vec<u8>, limit: usize) {
            if self.metadata {
                self.selected = if self.renamed {
                    self.source && self.destination
                } else {
                    self.ordinary
                };
                if self.selected {
                    append_bounded(output, &self.pending, limit);
                }
                self.pending.clear();
                self.metadata = false;
            }
        }
    }
    fn append_bounded(output: &mut Vec<u8>, bytes: &[u8], limit: usize) {
        let count = bytes.len().min((limit + 1).saturating_sub(output.len()));
        output.extend_from_slice(&bytes[..count]);
    }

    let path = quote_patch_path(&file.path);
    let source = file.original_path.as_deref().map(quote_patch_path);
    let ordinary = format!(
        "diff --git {} {}\n",
        quote_patch_path(&format!("a/{}", file.path)),
        quote_patch_path(&format!("b/{}", file.path))
    );
    let combined = format!("diff --cc {path}\n");
    let combined_long = format!("diff --combined {path}\n");
    // Bound each read even for a single enormous content line. An expected metadata
    // line fits in the first chunk, so partial unselected lines cannot match it.
    let chunk_limit = ordinary.len().max(source.as_ref().map_or(0, String::len)) + 128;
    let mut reader = BufReader::new(reader);
    let mut output = Vec::new();
    let mut record = Record::default();
    let mut beginning = true;
    loop {
        let mut chunk = Vec::new();
        if Read::by_ref(&mut reader)
            .take(chunk_limit as u64)
            .read_until(b'\n', &mut chunk)?
            == 0
        {
            record.finish_metadata(&mut output, limit);
            return Ok(output);
        }
        if beginning {
            if chunk.starts_with(b"diff --git ")
                || chunk.starts_with(b"diff --cc ")
                || chunk.starts_with(b"diff --combined ")
            {
                record.finish_metadata(&mut output, limit);
                record = Record {
                    metadata: true,
                    ordinary: chunk == ordinary.as_bytes()
                        || chunk == combined.as_bytes()
                        || chunk == combined_long.as_bytes(),
                    ..Record::default()
                };
            } else if record.metadata {
                let line = chunk.strip_suffix(b"\n").unwrap_or(&chunk);
                if let Some(from) = line
                    .strip_prefix(b"rename from ")
                    .or_else(|| line.strip_prefix(b"copy from "))
                {
                    record.renamed = true;
                    record.source = source.as_ref().is_none_or(|path| from == path.as_bytes());
                }
                if let Some(to) = line
                    .strip_prefix(b"rename to ")
                    .or_else(|| line.strip_prefix(b"copy to "))
                {
                    record.renamed = true;
                    record.destination = to == path.as_bytes();
                }
                if line.starts_with(b"--- ")
                    || line.starts_with(b"@@")
                    || line.starts_with(b"Binary files ")
                    || line == b"GIT binary patch"
                {
                    record.finish_metadata(&mut output, limit);
                }
            }
        }
        if record.metadata {
            append_bounded(&mut record.pending, &chunk, limit);
        } else if record.selected {
            append_bounded(&mut output, &chunk, limit);
        }
        if output.len() > limit {
            return Ok(output);
        }
        beginning = chunk.ends_with(b"\n");
    }
}

// Match Git's C-style patch-path quoting with core.quotePath=true. Spaces remain
// unquoted; rename/copy endpoint lines disambiguate those headers above.
fn quote_patch_path(path: &str) -> String {
    let mut quoted = String::new();
    let mut needs_quotes = false;
    for byte in path.bytes() {
        let escape = match byte {
            b'\x07' => Some("\\a"),
            b'\x08' => Some("\\b"),
            b'\t' => Some("\\t"),
            b'\n' => Some("\\n"),
            b'\x0b' => Some("\\v"),
            b'\x0c' => Some("\\f"),
            b'\r' => Some("\\r"),
            b'"' => Some("\\\""),
            b'\\' => Some("\\\\"),
            _ => None,
        };
        if let Some(escape) = escape {
            quoted.push_str(escape);
            needs_quotes = true;
        } else if !(0x20..0x7f).contains(&byte) {
            quoted.push_str(&format!("\\{byte:03o}"));
            needs_quotes = true;
        } else {
            quoted.push(byte as char);
        }
    }
    if needs_quotes {
        format!("\"{quoted}\"")
    } else {
        quoted
    }
}

fn validate_file_path(path: &str) -> Result<()> {
    if path.is_empty()
        || path.contains('\0')
        || Path::new(path)
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        bail!("Invalid repository-relative file path");
    }
    Ok(())
}

fn validate_worktree_path(root: &Path, path: &str) -> Result<()> {
    let mut full = root.to_path_buf();
    let mut components = Path::new(path).components().peekable();
    while let Some(component) = components.next() {
        full.push(component);
        match std::fs::symlink_metadata(&full) {
            Ok(metadata) if components.peek().is_some() => {
                if metadata.file_type().is_symlink() || !metadata.is_dir() {
                    bail!("File path has a non-directory or symbolic-link parent");
                }
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(error) => return Err(error).context("Cannot inspect file path"),
        }
    }
    Ok(())
}

fn untracked_file_details(root: &Path, path: &str) -> Result<String> {
    let mut text = format!("Untracked file contents: {}\n\n", display_path(path));
    text.push_str(&read_untracked_contents(root, path)?);
    Ok(bound_file_preview(text))
}

#[cfg(unix)]
fn read_untracked_contents(root: &Path, path: &str) -> Result<String> {
    use std::{
        ffi::CString,
        fs::{File, OpenOptions},
        os::{
            fd::{AsRawFd, FromRawFd},
            unix::{ffi::OsStrExt, fs::OpenOptionsExt},
        },
    };

    fn open_at(parent: &File, name: &CString, flags: libc::c_int) -> Result<File> {
        // SAFETY: parent owns a live descriptor and name is NUL-terminated. A successful
        // openat returns a new descriptor owned exclusively by the returned File.
        let fd = unsafe { libc::openat(parent.as_raw_fd(), name.as_ptr(), flags) };
        if fd < 0 {
            return Err(std::io::Error::last_os_error()).context("Cannot open untracked path");
        }
        Ok(unsafe { File::from_raw_fd(fd) })
    }

    let flags = libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK;
    let mut parent = OpenOptions::new()
        .read(true)
        .custom_flags(flags | libc::O_DIRECTORY)
        .open(root)
        .context("Cannot open repository root")?;
    let mut components = Path::new(path).components().peekable();
    while let Some(component) = components.next() {
        let name = CString::new(component.as_os_str().as_bytes())?;
        if components.peek().is_some() {
            // Each directory stays anchored by its descriptor, even if renamed while
            // previewing. Neither intermediate nor final symlinks can be followed.
            parent = open_at(&parent, &name, flags | libc::O_DIRECTORY)?;
            continue;
        }
        let mut metadata = std::mem::MaybeUninit::<libc::stat>::uninit();
        // SAFETY: metadata is writable, and fstatat initializes it on success.
        if unsafe {
            libc::fstatat(
                parent.as_raw_fd(),
                name.as_ptr(),
                metadata.as_mut_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        } < 0
        {
            return Err(std::io::Error::last_os_error()).context("Cannot inspect untracked file");
        }
        let kind = unsafe { metadata.assume_init() }.st_mode & libc::S_IFMT;
        if kind == libc::S_IFLNK {
            let mut bytes = vec![0; FILE_PREVIEW_LIMIT + 1];
            // readlinkat reads the link itself relative to the pinned parent, not its
            // target. Replacement by a non-symlink simply fails without reading it.
            let count = unsafe {
                libc::readlinkat(
                    parent.as_raw_fd(),
                    name.as_ptr(),
                    bytes.as_mut_ptr().cast(),
                    bytes.len(),
                )
            };
            if count < 0 {
                return Err(std::io::Error::last_os_error()).context("Cannot read symbolic link");
            }
            bytes.truncate(count as usize);
            return Ok(format!(
                "Symbolic link (target only; not followed): {}\n",
                display_path(&String::from_utf8_lossy(&bytes))
            ));
        }
        if kind == libc::S_IFDIR {
            return Ok("Directory contents are not previewed.\n".into());
        }
        if kind != libc::S_IFREG {
            return Ok("Special file contents are not previewed.\n".into());
        }
        // The entry may have changed after fstatat. NOFOLLOW rejects replacement
        // links, NONBLOCK prevents a replacement FIFO from blocking, and metadata
        // from the opened descriptor must confirm a regular file before any read.
        let opened = open_at(&parent, &name, flags)?;
        if !opened.metadata()?.is_file() {
            return Ok("Non-regular file contents are not previewed.\n".into());
        }
        let mut bytes = Vec::new();
        opened
            .take(FILE_PREVIEW_LIMIT as u64 + 1)
            .read_to_end(&mut bytes)?;
        return Ok(if bytes[..bytes.len().min(8192)].contains(&0) {
            "Binary file contents are not previewed.\n".into()
        } else {
            String::from_utf8_lossy(&bytes).into_owned()
        });
    }
    bail!("Invalid repository-relative file path")
}

#[cfg(not(unix))]
fn read_untracked_contents(_root: &Path, _path: &str) -> Result<String> {
    bail!("Safe no-follow untracked previews are not supported on this platform")
}

fn bound_file_preview(mut text: String) -> String {
    if text.len() > FILE_PREVIEW_LIMIT {
        let mut end = FILE_PREVIEW_LIMIT;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
        text.push_str(PREVIEW_TRUNCATED);
    }
    text
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

const DEMO_FILES: [(&str, &str, &str); 3] = [
    (
        "A",
        "src/curves.rs",
        "diff --git a/src/curves.rs b/src/curves.rs\nnew file mode 100644\n--- /dev/null\n+++ b/src/curves.rs\n@@ -0,0 +1,3 @@\n+pub fn stroke(from: Point, to: Point) {\n+    draw_bezier(from, to);\n+}\n",
    ),
    (
        "M",
        "src/graph.rs",
        "diff --git a/src/graph.rs b/src/graph.rs\n--- a/src/graph.rs\n+++ b/src/graph.rs\n@@ -12 +12,3 @@\n-    draw_ascii_edge(from, to);\n+    let curve = BranchCurve::new(from, to);\n+    renderer.stroke(curve, color);\n+    renderer.draw_node(commit.position());\n",
    ),
    (
        "D",
        "src/legacy_graph.rs",
        "diff --git a/src/legacy_graph.rs b/src/legacy_graph.rs\ndeleted file mode 100644\n--- a/src/legacy_graph.rs\n+++ /dev/null\n@@ -1,3 +0,0 @@\n-pub fn draw_ascii_edge(from: Point, to: Point) {\n-    draw_line(from, to);\n-}\n",
    ),
];

pub fn demo_changed_files() -> Vec<ChangedFile> {
    DEMO_FILES
        .iter()
        .map(|(status, path, _)| ChangedFile {
            status: (*status).into(),
            path: (*path).into(),
            original_path: None,
        })
        .collect()
}

pub fn demo_details(commit: &Commit) -> String {
    let patches = DEMO_FILES.map(|(_, _, patch)| patch).join("\n");
    format!(
        "commit {}\nAuthor: {}\nDate:   {}\n\n    {}\n\n src/curves.rs       | 3 +++\n src/graph.rs        | 4 +++-\n src/legacy_graph.rs | 3 ---\n 3 files changed, 6 insertions(+), 4 deletions(-)\n\n{patches}\n[Demo data · no repository changes]",
        commit.oid, commit.author, commit.date, commit.subject,
    )
}

pub fn demo_file_details(path: &str) -> String {
    let (_, _, patch) = DEMO_FILES
        .iter()
        .find(|(_, name, _)| *name == path)
        .expect("selected file belongs to the demo");
    format!("{patch}\n[Demo data · no repository changes]")
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
            reference: if name == "origin/main" {
                format!("refs/remotes/{name}")
            } else {
                format!("refs/heads/{name}")
            },
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
