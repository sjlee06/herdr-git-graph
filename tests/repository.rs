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
    fs::write(root.join("base.txt"), "staged\n").unwrap();
    command(root, &["add", "base.txt"]);
    fs::write(root.join("base.txt"), "unstaged\n").unwrap();
    let diff = git::details(root, git::WORKTREE_OID).unwrap();
    assert!(diff.contains("+staged") && diff.contains("+unstaged"));
}

#[test]
fn uncommitted_row_combines_status_and_keeps_both_sides_of_partial_staging() {
    let dir = init();
    let root = dir.path();
    let head = commit(root, "tracked.txt", "initial");
    command(root, &["branch", "other"]);
    fs::write(root.join(".gitignore"), "ignored/\n").unwrap();
    command(root, &["add", ".gitignore"]);
    fs::create_dir(root.join("ignored")).unwrap();
    fs::write(root.join("ignored/artifact"), "ignored").unwrap();
    fs::write(root.join("tracked.txt"), "staged text\n").unwrap();
    command(root, &["add", "tracked.txt"]);
    fs::write(root.join("tracked.txt"), "unstaged text\n").unwrap();
    fs::write(root.join("새 파일\nname.txt"), "untracked\n").unwrap();
    command(root, &["config", "status.showUntrackedFiles", "no"]);
    let before = command(root, &["status", "--porcelain=v1", "-uall"]);
    let index_before = fs::read(root.join(".git/index")).unwrap();
    let repo = git::load(root, None, 1).unwrap();
    assert_eq!(repo.commit_count(), 1);
    assert_eq!(repo.commits.len(), 2);
    assert!(repo.commits[0].is_worktree());
    assert_eq!(repo.commits[0].parents, vec![head]);
    assert_eq!(
        (
            repo.worktree.staged,
            repo.worktree.unstaged,
            repo.worktree.untracked
        ),
        (2, 1, 1)
    );
    assert!(
        !repo
            .worktree
            .files
            .iter()
            .any(|f| f.path.starts_with("ignored/"))
    );
    let graph = Graph::build(&repo.commits);
    assert_eq!(graph.rows[0].column, graph.rows[1].column);
    assert!(graph.rows[0].uncommitted);
    let details = git::details(root, git::WORKTREE_OID).unwrap();
    assert!(details.contains("+staged text"));
    assert!(details.contains("+unstaged text"));
    assert!(details.contains("새 파일\\nname.txt"));
    assert_eq!(fs::read(root.join(".git/index")).unwrap(), index_before);
    assert_eq!(
        command(root, &["status", "--porcelain=v1", "-uall"]),
        before
    );
    assert!(git::load(root, Some("refs/heads/main"), 1).unwrap().commits[0].is_worktree());
    assert!(
        !git::load(root, Some("refs/heads/other"), 1)
            .unwrap()
            .commits[0]
            .is_worktree()
    );
}

#[test]
fn unborn_repository_shows_new_files_and_staged_diff_without_a_parent() {
    let dir = init();
    let root = dir.path();
    fs::write(root.join("new.txt"), "first contents\n").unwrap();
    let repo = git::load(root, None, 100).unwrap();
    assert_eq!(repo.commit_count(), 0);
    assert_eq!(repo.worktree.untracked, 1);
    assert!(repo.commits[0].parents.is_empty());
    command(root, &["add", "new.txt"]);
    let repo = git::refresh(&repo, None, 100).unwrap();
    assert_eq!(repo.worktree.staged, 1);
    assert!(
        git::details(root, git::WORKTREE_OID)
            .unwrap()
            .contains("+first contents")
    );
    command(
        root,
        &["-c", "commit.gpgsign=false", "commit", "-m", "first"],
    );
    let repo = git::refresh(&repo, None, 100).unwrap();
    assert!(repo.worktree.files.is_empty());
    assert_eq!(repo.commit_count(), 1);
    assert!(!repo.commits[0].is_worktree());
}

#[test]
fn rename_delete_and_conflict_statuses_are_preserved() {
    let dir = init();
    let root = dir.path();
    commit(root, "old.txt", "original");
    commit(root, "delete.txt", "remove me");
    command(root, &["mv", "old.txt", "renamed file.txt"]);
    fs::remove_file(root.join("delete.txt")).unwrap();
    let tree = git::working_tree(root).unwrap();
    assert!(
        tree.files
            .iter()
            .any(|f| f.path == "renamed file.txt" && f.original_path.as_deref() == Some("old.txt"))
    );
    assert!(
        tree.files
            .iter()
            .any(|f| f.path == "delete.txt" && f.status == " D")
    );
    command(root, &["add", "-A"]);
    command(
        root,
        &["-c", "commit.gpgsign=false", "commit", "-m", "rename"],
    );
    command(root, &["checkout", "-b", "conflict"]);
    commit(root, "renamed file.txt", "other side");
    command(root, &["checkout", "main"]);
    commit(root, "renamed file.txt", "main side");
    let merge = Command::new("git")
        .arg("-C")
        .arg(root)
        .args([
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "merge",
            "conflict",
        ])
        .output()
        .unwrap();
    assert!(!merge.status.success());
    let tree = git::working_tree(root).unwrap();
    assert_eq!(tree.conflicts, 1);
    assert!(
        git::working_tree_details(root, &tree)
            .unwrap()
            .contains("UU renamed file.txt")
    );
}

#[test]
fn linked_worktree_changes_attach_to_its_own_head_and_detached_head() {
    let dir = init();
    let root = dir.path();
    let head = commit(root, "tracked.txt", "initial");
    let outside = tempfile::tempdir().unwrap();
    let linked = outside.path().join("linked");
    command(
        root,
        &["worktree", "add", "-b", "linked", linked.to_str().unwrap()],
    );
    fs::write(linked.join("tracked.txt"), "linked only\n").unwrap();
    let repo = git::load(&linked, None, 100).unwrap();
    assert_eq!(repo.commits[0].parents, vec![head.clone()]);
    assert_eq!(repo.worktree.unstaged, 1);
    assert!(
        git::load(root, None, 100)
            .unwrap()
            .worktree
            .files
            .is_empty()
    );
    command(&linked, &["checkout", "--detach"]);
    let detached = git::refresh(&repo, None, 100).unwrap();
    assert_eq!(detached.head, "detached HEAD");
    assert_eq!(detached.commits[0].parents, vec![head]);
}

#[test]
fn refresh_detects_new_commits_tags_and_branch_switches() {
    let dir = init();
    let root = dir.path();
    commit(root, "tracked.txt", "initial");
    let repo = git::load(root, None, 100).unwrap();
    assert_eq!(repo, git::refresh(&repo, None, 100).unwrap());
    let head = commit(root, "tracked.txt", "next");
    command(root, &["tag", "new-tag"]);
    command(root, &["checkout", "-b", "new-branch"]);
    let repo = git::refresh(&repo, None, 100).unwrap();
    assert_eq!(repo.head, "new-branch");
    assert_eq!(repo.commits[0].oid, head);
    assert!(repo.commits[0].refs.contains("new-tag"));
    assert_eq!(repo.commit_count(), 2);
    assert_eq!(git::refresh(&repo, None, 1).unwrap().commit_count(), 1);
}

#[test]
fn worker_refreshes_diff_when_an_already_modified_file_changes_again() {
    use herdr_git_graph::app::{Request, Response, Worker};
    use std::time::Duration;
    let dir = init();
    let root = dir.path();
    commit(root, "tracked.txt", "initial");
    let worker = Worker::start(root.into(), 100);
    let mut last_repo = None;
    for contents in ["first edit\n", "second edit\n"] {
        fs::write(root.join("tracked.txt"), contents).unwrap();
        worker
            .sender
            .send(Request::Refresh {
                reference: None,
                id: 0,
                detail: true,
            })
            .unwrap();
        match worker
            .receiver
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
        {
            Response::Refreshed {
                result: Ok(repo),
                detail: Some(Ok(diff)),
                ..
            } => {
                assert!(diff.contains(&format!("+{contents}")));
                if let Some(previous) = &last_repo {
                    assert_eq!(previous, &repo);
                }
                last_repo = Some(repo);
            }
            _ => panic!("Expected repository and working tree detail"),
        }
    }
}

#[test]
fn large_uncommitted_patches_are_bounded() {
    let dir = init();
    let root = dir.path();
    commit(root, "large.txt", "small");
    fs::write(root.join("large.txt"), "large new line\n".repeat(100_000)).unwrap();
    let diff = git::details(root, git::WORKTREE_OID).unwrap();
    assert!(diff.contains("Diff preview truncated at 512 KiB"));
    assert!(diff.len() < 520 * 1024);
}
