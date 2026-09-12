use herdr_git_graph::{git, graph::Graph};
use std::{fs, path::Path, process::Command};
use tempfile::TempDir;

fn command(repo: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "Test Author")
        .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
        .env("GIT_COMMITTER_NAME", "Test Author")
        .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

fn init() -> TempDir {
    let dir = tempfile::Builder::new()
        .prefix("git graph with spaces ")
        .tempdir()
        .unwrap();
    command(dir.path(), &["init", "-b", "main"]);
    dir
}

fn commit(repo: &Path, name: &str, message: &str) -> String {
    fs::write(repo.join(name), format!("{message}\n")).unwrap();
    command(repo, &["add", "--", name]);
    command(
        repo,
        &["-c", "commit.gpgsign=false", "commit", "-m", message],
    );
    command(repo, &["rev-parse", "HEAD"])
}

#[test]
fn real_merge_filter_diff_detached_and_worktree() {
    let dir = init();
    let root = dir.path();
    let initial = commit(root, "base.txt", "initial");
    command(root, &["checkout", "-b", "feature/search"]);
    let feature = commit(root, "검색.txt", "한글 검색\t지원\n\nmessage body");
    command(root, &["checkout", "main"]);
    commit(root, "main.txt", "main update");
    command(
        root,
        &[
            "-c",
            "commit.gpgsign=false",
            "merge",
            "--no-ff",
            "feature/search",
            "-m",
            "merge search",
        ],
    );
    command(root, &["tag", "v0.1.0"]);
    command(root, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    let before = command(root, &["status", "--porcelain=v1"]);
    let repo = git::load(root, None, 100).unwrap();
    assert_eq!(repo.commits.len(), 4);
    assert_eq!(repo.commits[0].parents.len(), 2);
    assert!(repo.commits[0].refs.contains("v0.1.0"));
    assert!(repo.branches.iter().any(|b| b.name == "origin/main"));
    assert!(repo.commits.iter().any(|c| c.subject.contains("한글 검색")));
    let graph = Graph::build(&repo.commits);
    for (i, c) in repo.commits.iter().enumerate() {
        let row = &graph.rows[i];
        let parents: Vec<_> = row
            .edges
            .iter()
            .filter(|e| e.from == row.column)
            .map(|e| row.below[e.to].as_ref().unwrap().oid.as_str())
            .collect();
        assert_eq!(
            parents,
            c.parents.iter().map(String::as_str).collect::<Vec<_>>()
        );
    }
    let filtered = git::load(root, Some("refs/heads/feature/search"), 100).unwrap();
    assert_eq!(filtered.commits.len(), 2);
    assert_eq!(filtered.commits[0].oid, feature);
    assert!(git::load(root, Some("--output=/tmp/should-not-exist"), 100).is_err());
    let diff = git::details(root, &feature).unwrap();
    assert!(diff.contains("+한글 검색"));
    assert_eq!(command(root, &["status", "--porcelain=v1"]), before);
    command(root, &["checkout", "--detach", &initial]);
    let detached = commit(root, "detached.txt", "detached commit");
    assert!(
        git::load(root, None, 100)
            .unwrap()
            .commits
            .iter()
            .any(|c| c.oid == detached)
    );
    let linked = root.join("linked worktree");
    command(
        root,
        &[
            "worktree",
            "add",
            linked.to_str().unwrap(),
            "feature/search",
        ],
    );
    fs::create_dir(linked.join("nested")).unwrap();
    let discovered = git::discover(&linked.join("nested")).unwrap();
    assert_eq!(
        discovered.canonicalize().unwrap(),
        linked.canonicalize().unwrap()
    );
    assert_eq!(
        git::load(&discovered, None, 100).unwrap().head,
        "feature/search"
    );
}

#[test]
fn empty_repo_and_empty_subject_are_supported() {
    let dir = init();
    assert!(git::load(dir.path(), None, 100).unwrap().commits.is_empty());
    command(
        dir.path(),
        &[
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--allow-empty",
            "--allow-empty-message",
            "-m",
            "",
        ],
    );
    let repo = git::load(dir.path(), None, 100).unwrap();
    assert_eq!(repo.commits.len(), 1);
    assert_eq!(repo.commits[0].subject, "");
}

#[test]
fn shallow_history_and_commit_limit_do_not_invent_edges() {
    let source = init();
    for i in 0..5 {
        commit(source.path(), "file.txt", &format!("commit {i}"));
    }
    let destination = tempfile::tempdir().unwrap();
    let cloned = destination.path().join("shallow");
    command(
        destination.path(),
        &[
            "clone",
            "--depth",
            "2",
            &format!("file://{}", source.path().display()),
            cloned.to_str().unwrap(),
        ],
    );
    let repo = git::load(&cloned, None, 100).unwrap();
    assert_eq!(repo.commits.len(), 2);
    assert!(repo.commits.last().unwrap().parents.is_empty());
    let limited = git::load(source.path(), None, 2).unwrap();
    assert_eq!(limited.commits.len(), 2);
    assert!(!limited.commits.last().unwrap().parents.is_empty());
}

#[test]
fn external_diff_is_never_executed() {
    let dir = init();
    let root = dir.path();
    commit(root, "base.txt", "initial");
    let oid = commit(root, "base.txt", "changed");
    command(
        root,
        &[
            "config",
            "diff.external",
            "definitely-not-an-executable-git-graph-test",
        ],
    );
    assert!(git::details(root, &oid).unwrap().contains("+changed"));
}
