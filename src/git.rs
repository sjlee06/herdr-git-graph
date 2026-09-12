use anyhow::{Context, Result, bail};
use std::{
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Debug)]
pub struct Commit {
    pub oid: String,
    pub parents: Vec<String>,
    pub author: String,
    pub date: String,
    pub refs: String,
    pub subject: String,
}

#[derive(Clone, Debug)]
pub struct Branch {
    pub reference: String,
    pub name: String,
    pub oid: String,
}

#[derive(Clone, Debug)]
pub struct Repository {
    pub root: PathBuf,
    pub head: String,
    pub commits: Vec<Commit>,
    pub branches: Vec<Branch>,
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
        if args.first() == Some(&"show") {
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
    let head = run(root, &["symbolic-ref", "--short", "HEAD"], 65536)
        .unwrap_or_else(|_| "detached HEAD".into())
        .trim()
        .to_owned();
    let raw_refs = run(
        root,
        &[
            "for-each-ref",
            "--sort=refname",
            "--format=%(refname)%00%(refname:short)%00%(objectname)%00%(symref)",
            "refs/heads",
            "refs/remotes",
        ],
        4 * 1024 * 1024,
    )?;
    let branches = raw_refs
        .lines()
        .filter_map(|line| {
            let parts: Vec<_> = line.split('\0').collect();
            (parts.len() == 4 && parts[3].is_empty()).then(|| Branch {
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
    let valid_head = run(root, &["rev-parse", "--verify", "HEAD^{commit}"], 65536).is_ok();
    if let Some(reference) = reference {
        // Public callers may supply a revision; disallow option injection.
        if reference.starts_with('-') {
            bail!("잘못된 Git ref입니다.");
        }
        args.push(reference);
    } else {
        args.push("--all");
        if valid_head {
            args.push("HEAD");
        }
    }
    args.push("--");
    let commits = parse_log(&run(root, &args, 32 * 1024 * 1024)?)?;
    Ok(Repository {
        root: root.into(),
        head,
        commits,
        branches,
    })
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
