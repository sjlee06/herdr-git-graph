use herdr_git_graph::git::{self, ChangedFile, WORKTREE_OID};
use std::{fs, path::Path, process::Command};
use tempfile::TempDir;

fn command(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(["--no-optional-locks", "--literal-pathspecs", "-C"])
        .arg(root)
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
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

fn init() -> TempDir {
    let dir = tempfile::Builder::new()
        .prefix("file diff with spaces ")
        .tempdir()
        .unwrap();
    command(dir.path(), &["init", "-b", "main"]);
    dir
}

fn commit_all(root: &Path, message: &str) -> String {
    command(root, &["add", "--all", "--", "."]);
    command(
        root,
        &["-c", "commit.gpgsign=false", "commit", "-m", message],
    );
    command(root, &["rev-parse", "HEAD"])
}

fn selected(root: &Path, oid: &str, path: &str) -> ChangedFile {
    git::changed_files(root, oid)
        .unwrap()
        .into_iter()
        .find(|file| file.path == path)
        .unwrap_or_else(|| panic!("missing changed file {path:?}"))
}

fn file(path: &str, status: &str) -> ChangedFile {
    ChangedFile {
        path: path.into(),
        status: status.into(),
        original_path: None,
    }
}

fn snapshot(root: &Path) -> (Option<Vec<u8>>, String) {
    (
        fs::read(root.join(".git/index")).ok(),
        command(root, &["status", "--porcelain=v1", "-z", "-uall"]),
    )
}

#[test]
fn root_and_ordinary_commits_show_only_the_selected_patch() {
    let dir = init();
    let root = dir.path();
    fs::write(root.join("selected.txt"), "selected root contents\n").unwrap();
    fs::write(root.join("other.txt"), "unrelated root contents\n").unwrap();
    let initial = commit_all(root, "root includes both files");
    let patch =
        git::file_details(root, &initial, &selected(root, &initial, "selected.txt")).unwrap();
    assert!(patch.contains("diff --git a/selected.txt b/selected.txt"));
    assert!(patch.contains("new file mode"));
    assert!(patch.contains("+selected root contents"));
    assert!(!patch.contains("other.txt") && !patch.contains("unrelated"));
    assert!(!patch.contains("root includes both files"));

    fs::write(root.join("selected.txt"), "selected ordinary contents\n").unwrap();
    fs::write(root.join("other.txt"), "unrelated ordinary contents\n").unwrap();
    let oid = commit_all(root, "ordinary includes both files");
    let before = snapshot(root);
    let patch = git::file_details(root, &oid, &selected(root, &oid, "selected.txt")).unwrap();
    assert!(patch.contains("-selected root contents"));
    assert!(patch.contains("+selected ordinary contents"));
    assert!(!patch.contains("other.txt") && !patch.contains("unrelated"));
    assert_eq!(snapshot(root), before);
}

#[test]
fn merge_patch_compares_only_with_the_first_parent() {
    let dir = init();
    let root = dir.path();
    fs::write(root.join("selected.txt"), "base text\n").unwrap();
    commit_all(root, "base");
    command(root, &["checkout", "-b", "feature"]);
    fs::write(root.join("selected.txt"), "feature text\n").unwrap();
    fs::write(root.join("feature-other.txt"), "unrelated feature\n").unwrap();
    commit_all(root, "feature");
    command(root, &["checkout", "main"]);
    fs::write(root.join("main-only.txt"), "main side change\n").unwrap();
    commit_all(root, "main");
    command(
        root,
        &[
            "-c",
            "commit.gpgsign=false",
            "merge",
            "--no-ff",
            "feature",
            "-m",
            "merge feature",
        ],
    );
    let oid = command(root, &["rev-parse", "HEAD"]);
    let patch = git::file_details(root, &oid, &selected(root, &oid, "selected.txt")).unwrap();
    assert!(patch.contains("-base text") && patch.contains("+feature text"));
    assert!(!patch.contains("feature-other.txt") && !patch.contains("main-only.txt"));
    assert!(!patch.contains("diff --cc"));
}

#[test]
fn renamed_deleted_and_binary_committed_files_have_selected_previews() {
    let dir = init();
    let root = dir.path();
    let contents = "rename line\n".repeat(40);
    fs::write(root.join("old name.txt"), &contents).unwrap();
    fs::write(root.join("deleted.txt"), "deleted contents\n").unwrap();
    fs::write(root.join("binary.dat"), b"before\0binary\n").unwrap();
    fs::write(root.join("other.txt"), "other before\n").unwrap();
    commit_all(root, "base");
    fs::rename(root.join("old name.txt"), root.join("new name.txt")).unwrap();
    fs::write(
        root.join("new name.txt"),
        format!("{contents}rename addition\n"),
    )
    .unwrap();
    fs::remove_file(root.join("deleted.txt")).unwrap();
    fs::write(root.join("binary.dat"), b"after\0binary\n").unwrap();
    fs::write(root.join("other.txt"), "unrelated change\n").unwrap();
    let oid = commit_all(root, "rename delete binary");
    let renamed = selected(root, &oid, "new name.txt");
    assert_eq!(renamed.original_path.as_deref(), Some("old name.txt"));
    let patch = git::file_details(root, &oid, &renamed).unwrap();
    assert!(patch.contains("rename from old name.txt"));
    assert!(patch.contains("rename to new name.txt"));
    assert!(patch.contains("+rename addition"));
    assert!(!patch.contains("other.txt") && !patch.contains("deleted.txt"));
    let deleted = git::file_details(root, &oid, &selected(root, &oid, "deleted.txt")).unwrap();
    assert!(deleted.contains("deleted file mode") && deleted.contains("-deleted contents"));
    assert!(!deleted.contains("new name.txt"));
    let binary = git::file_details(root, &oid, &selected(root, &oid, "binary.dat")).unwrap();
    assert!(binary.contains("Binary files") && binary.contains("binary.dat"));
    assert!(!binary.contains("unrelated change") && !binary.contains('\0'));
}

#[test]
fn worktree_separates_staged_and_unstaged_without_mutation() {
    let dir = init();
    let root = dir.path();
    fs::write(root.join("selected.txt"), "base selected\n").unwrap();
    fs::write(root.join("other.txt"), "base other\n").unwrap();
    commit_all(root, "base");
    fs::write(root.join("selected.txt"), "staged selected\n").unwrap();
    fs::write(root.join("other.txt"), "staged unrelated\n").unwrap();
    command(root, &["add", "--all"]);
    fs::write(root.join("selected.txt"), "unstaged selected\n").unwrap();
    fs::write(root.join("other.txt"), "unstaged unrelated\n").unwrap();
    let before = snapshot(root);
    let selected = selected(root, WORKTREE_OID, "selected.txt");
    assert_eq!(selected.status, "MM");
    let patch = git::file_details(root, WORKTREE_OID, &selected).unwrap();
    let (staged, unstaged) = patch.split_once("Unstaged changes\n").unwrap();
    assert!(staged.starts_with("Staged changes\n"));
    assert!(staged.contains("-base selected") && staged.contains("+staged selected"));
    assert!(!staged.contains("unstaged selected"));
    assert!(unstaged.contains("-staged selected") && unstaged.contains("+unstaged selected"));
    assert!(!patch.contains("other.txt") && !patch.contains("unrelated"));
    assert_eq!(snapshot(root), before);
    assert_eq!(
        fs::read_to_string(root.join("selected.txt")).unwrap(),
        "unstaged selected\n"
    );
}

#[test]
fn worktree_rename_includes_both_paths_and_deleted_binary_files_work() {
    let dir = init();
    let root = dir.path();
    fs::write(root.join("old.txt"), "rename base\n".repeat(40)).unwrap();
    fs::write(root.join("deleted.txt"), "gone from worktree\n").unwrap();
    fs::write(root.join("binary.dat"), b"before\0binary").unwrap();
    commit_all(root, "base");
    command(root, &["mv", "old.txt", "new.txt"]);
    fs::write(root.join("new.txt"), "rename worktree\n".repeat(40)).unwrap();
    fs::remove_file(root.join("deleted.txt")).unwrap();
    fs::write(root.join("binary.dat"), b"after\0binary").unwrap();
    let before = snapshot(root);
    let renamed = selected(root, WORKTREE_OID, "new.txt");
    assert_eq!(renamed.original_path.as_deref(), Some("old.txt"));
    let patch = git::file_details(root, WORKTREE_OID, &renamed).unwrap();
    let (staged, unstaged) = patch.split_once("Unstaged changes\n").unwrap();
    assert!(staged.contains("rename from old.txt") && staged.contains("rename to new.txt"));
    assert!(unstaged.contains("+rename worktree"));
    assert!(!patch.contains("deleted.txt") && !patch.contains("binary.dat"));
    let deleted = git::file_details(
        root,
        WORKTREE_OID,
        &selected(root, WORKTREE_OID, "deleted.txt"),
    )
    .unwrap();
    assert!(deleted.contains("deleted file mode") && deleted.contains("-gone from worktree"));
    let binary = git::file_details(
        root,
        WORKTREE_OID,
        &selected(root, WORKTREE_OID, "binary.dat"),
    )
    .unwrap();
    assert!(binary.contains("Binary files"));
    assert_eq!(snapshot(root), before);
}

#[test]
fn unborn_staged_and_untracked_previews_are_read_only() {
    let dir = init();
    let root = dir.path();
    fs::write(
        root.join("new.txt"),
        "untracked text\nwithout final newline",
    )
    .unwrap();
    fs::write(root.join("other.txt"), "unrelated untracked\n").unwrap();
    let before = snapshot(root);
    assert!(before.0.is_none());
    let patch =
        git::file_details(root, WORKTREE_OID, &selected(root, WORKTREE_OID, "new.txt")).unwrap();
    assert!(patch.contains("Untracked file contents"));
    assert!(patch.contains("untracked text\nwithout final newline"));
    assert!(!patch.contains("other.txt") && !patch.contains("unrelated untracked"));
    assert_eq!(snapshot(root), before);
    command(root, &["add", "new.txt"]);
    let before = snapshot(root);
    let patch =
        git::file_details(root, WORKTREE_OID, &selected(root, WORKTREE_OID, "new.txt")).unwrap();
    assert!(patch.contains("Staged changes") && patch.contains("+untracked text"));
    assert!(patch.contains("Unstaged changes\n(none)"));
    assert_eq!(snapshot(root), before);
}

#[test]
fn literal_special_names_never_expand_or_become_options() {
    let dir = init();
    let root = dir.path();
    let names = [
        "*.txt",
        "[ab].txt",
        ":(glob)*",
        ":(top)other.txt",
        "--output=unexpected",
        "-leading.txt",
        "한글 space\tand\nnewline.txt",
        "back\\slash.txt",
        "quoted\"name\r.txt",
        "\u{7}bell.txt",
        "embedded b/segment.txt",
    ];
    for (i, name) in names.iter().enumerate() {
        fs::create_dir_all(root.join(name).parent().unwrap()).unwrap();
        fs::write(root.join(name), format!("selected marker {i}\n")).unwrap();
    }
    fs::write(root.join("a.txt"), "unrelated marker\n").unwrap();
    fs::write(root.join("other.txt"), "unrelated marker\n").unwrap();
    let initial = commit_all(root, "special names");
    for (i, name) in names.iter().enumerate() {
        let patch = git::file_details(root, &initial, &selected(root, &initial, name)).unwrap();
        assert!(
            patch.contains(&format!("+selected marker {i}")),
            "{name:?}: {patch}"
        );
        assert_eq!(patch.matches("diff --git ").count(), 1, "{name:?}: {patch}");
        assert!(!patch.contains("unrelated marker"));
        fs::write(root.join(name), format!("staged marker {i}\n")).unwrap();
    }
    command(root, &["add", "--all"]);
    for (i, name) in names.iter().enumerate() {
        fs::write(root.join(name), format!("unstaged marker {i}\n")).unwrap();
    }
    let before = snapshot(root);
    for (i, name) in names.iter().enumerate() {
        let patch =
            git::file_details(root, WORKTREE_OID, &selected(root, WORKTREE_OID, name)).unwrap();
        assert!(
            patch.contains(&format!("+staged marker {i}")),
            "{name:?}: {patch}"
        );
        assert!(
            patch.contains(&format!("+unstaged marker {i}")),
            "{name:?}: {patch}"
        );
        assert_eq!(patch.matches("diff --git ").count(), 2, "{name:?}: {patch}");
        assert!(!patch.contains("unrelated marker"));
    }
    assert_eq!(snapshot(root), before);
    assert!(!root.join("unexpected").exists());
}

#[test]
fn invalid_revisions_and_unsafe_paths_fail_without_mutation() {
    let dir = init();
    let root = dir.path();
    fs::write(root.join("selected.txt"), "tracked text\n").unwrap();
    let oid = commit_all(root, "base");
    let before = snapshot(root);
    for revision in ["", "HEAD", "--output=bad", "a..b", "deadbeef", "../HEAD"] {
        assert!(
            git::file_details(root, revision, &file("selected.txt", "M")).is_err(),
            "{revision:?}"
        );
    }
    let blob = command(root, &["rev-parse", "HEAD:selected.txt"]);
    let tree = command(root, &["rev-parse", "HEAD^{tree}"]);
    assert!(git::file_details(root, &blob, &file("selected.txt", "M")).is_err());
    assert!(git::file_details(root, &tree, &file("selected.txt", "M")).is_err());
    for path in [
        "",
        ".",
        "..",
        "../outside",
        "nested/../../outside",
        "/etc/passwd",
        "nul\0name",
    ] {
        for revision in [&oid, WORKTREE_OID] {
            assert!(
                git::file_details(root, revision, &file(path, "??")).is_err(),
                "{path:?}"
            );
            let mut renamed = file("selected.txt", "R");
            renamed.original_path = Some(path.into());
            assert!(
                git::file_details(root, revision, &renamed).is_err(),
                "{path:?}"
            );
        }
    }
    assert_eq!(snapshot(root), before);
}

#[test]
fn external_diff_textconv_and_color_are_disabled() {
    let dir = init();
    let root = dir.path();
    fs::write(root.join("selected.txt"), "base plain text\n").unwrap();
    fs::write(root.join(".gitattributes"), "*.txt diff=unsafe\n").unwrap();
    commit_all(root, "base");
    fs::write(root.join("selected.txt"), "commit plain text\n").unwrap();
    let oid = commit_all(root, "change");
    for key in [
        "diff.external",
        "diff.unsafe.command",
        "diff.unsafe.textconv",
    ] {
        command(root, &["config", key, "definitely-not-a-diff-command"]);
    }
    command(root, &["config", "color.ui", "always"]);
    command(root, &["config", "color.diff", "always"]);
    command(root, &["config", "core.quotePath", "false"]);
    command(root, &["config", "diff.noprefix", "true"]);
    command(root, &["config", "diff.mnemonicPrefix", "true"]);
    command(root, &["config", "diff.relative", "true"]);
    command(root, &["config", "diff.submodule", "diff"]);
    let patch = git::file_details(root, &oid, &selected(root, &oid, "selected.txt")).unwrap();
    assert!(patch.contains("+commit plain text") && !patch.contains('\u{1b}'));
    fs::write(root.join("selected.txt"), "working plain text\n").unwrap();
    let before = snapshot(root);
    let patch = git::file_details(
        root,
        WORKTREE_OID,
        &selected(root, WORKTREE_OID, "selected.txt"),
    )
    .unwrap();
    assert!(patch.contains("+working plain text") && !patch.contains('\u{1b}'));
    assert_eq!(snapshot(root), before);
}

#[test]
fn replacing_a_file_with_a_directory_does_not_preview_descendants() {
    let dir = init();
    let root = dir.path();
    fs::write(root.join("selected"), "selected deleted contents\n").unwrap();
    commit_all(root, "base file");
    fs::remove_file(root.join("selected")).unwrap();
    fs::create_dir(root.join("selected")).unwrap();
    // A large descendant must neither leak nor exhaust the selected patch budget.
    fs::write(
        root.join("selected/other.txt"),
        "unrelated child contents\n".repeat(30_000),
    )
    .unwrap();
    command(root, &["add", "--all"]);
    let deleted = selected(root, WORKTREE_OID, "selected");
    let before = snapshot(root);
    let patch = git::file_details(root, WORKTREE_OID, &deleted).unwrap();
    assert!(patch.contains("-selected deleted contents"));
    assert!(!patch.contains("other.txt") && !patch.contains("unrelated child"));
    assert_eq!(snapshot(root), before);
    let oid = commit_all(root, "file replaced by directory");
    let patch = git::file_details(root, &oid, &selected(root, &oid, "selected")).unwrap();
    assert!(patch.contains("-selected deleted contents"));
    assert!(!patch.contains("other.txt") && !patch.contains("unrelated child"));
    assert!(!patch.contains("truncated"));
}

#[test]
fn reused_rename_source_does_not_preview_the_unrelated_new_file() {
    let dir = init();
    let root = dir.path();
    fs::write(
        root.join("old.txt"),
        "selected rename contents\n".repeat(40),
    )
    .unwrap();
    commit_all(root, "base rename source");
    command(root, &["mv", "old.txt", "new.txt"]);
    let renamed = selected(root, WORKTREE_OID, "new.txt");
    assert_eq!(renamed.original_path.as_deref(), Some("old.txt"));
    fs::write(
        root.join("old.txt"),
        "unrelated source reuse\n".repeat(30_000),
    )
    .unwrap();
    command(root, &["add", "old.txt"]);
    let before = snapshot(root);
    let patch = git::file_details(root, WORKTREE_OID, &renamed).unwrap();
    assert!(patch.contains("selected rename contents") || patch.contains("rename to new.txt"));
    assert!(!patch.contains("unrelated source reuse"));
    assert!(!patch.contains("diff --git a/old.txt b/old.txt"));
    assert!(!patch.contains("truncated"));
    assert_eq!(snapshot(root), before);
    let oid = commit_all(root, "source reused after rename");
    let patch = git::file_details(root, &oid, &renamed).unwrap();
    assert!(patch.contains("selected rename contents") || patch.contains("rename to new.txt"));
    assert!(!patch.contains("unrelated source reuse"));
    assert!(!patch.contains("diff --git a/old.txt b/old.txt"));
    assert!(!patch.contains("truncated"));
}

#[test]
fn pure_rename_with_quoted_endpoints_excludes_reused_source_directory() {
    let dir = init();
    let root = dir.path();
    let old = "old b/quoted\"source\n.txt";
    let new = "new b/quoted\"destination\n.txt";
    fs::create_dir(root.join("old b")).unwrap();
    fs::create_dir(root.join("new b")).unwrap();
    fs::write(root.join(old), "selected rename body\n").unwrap();
    commit_all(root, "base");
    command(root, &["mv", "--", old, new]);
    let renamed = selected(root, WORKTREE_OID, new);
    fs::create_dir(root.join(old)).unwrap();
    fs::write(
        root.join(old).join("child.txt"),
        "unrelated reused source child\n",
    )
    .unwrap();
    command(root, &["add", "--all"]);
    let before = snapshot(root);
    let patch = git::file_details(root, WORKTREE_OID, &renamed).unwrap();
    assert!(patch.contains("rename from ") && patch.contains("rename to "));
    assert!(!patch.contains("unrelated reused source") && !patch.contains("child.txt"));
    assert_eq!(snapshot(root), before);
    let oid = commit_all(root, "pure rename and source directory");
    let patch = git::file_details(root, &oid, &renamed).unwrap();
    assert!(patch.contains("rename from ") && patch.contains("rename to "));
    assert!(!patch.contains("unrelated reused source") && !patch.contains("child.txt"));
}

#[cfg(unix)]
#[test]
fn mode_only_selected_patch_is_retained_without_a_hunk() {
    use std::os::unix::fs::PermissionsExt;

    let dir = init();
    let root = dir.path();
    fs::write(root.join("selected.sh"), "echo selected\n").unwrap();
    commit_all(root, "base");
    command(root, &["config", "core.fileMode", "true"]);
    fs::set_permissions(root.join("selected.sh"), fs::Permissions::from_mode(0o755)).unwrap();
    let before = snapshot(root);
    let patch = git::file_details(root, WORKTREE_OID, &file("selected.sh", " M")).unwrap();
    assert!(patch.contains("old mode 100644") && patch.contains("new mode 100755"));
    assert_eq!(snapshot(root), before);
    let oid = commit_all(root, "mode change");
    let patch = git::file_details(root, &oid, &selected(root, &oid, "selected.sh")).unwrap();
    assert!(patch.contains("old mode 100644") && patch.contains("new mode 100755"));
}

#[test]
fn large_commit_worktree_and_untracked_previews_are_bounded() {
    let dir = init();
    let root = dir.path();
    let large = "large preview line\n".repeat(50_000);
    fs::write(root.join("large.txt"), &large).unwrap();
    let oid = commit_all(root, "large root");
    let patch = git::file_details(root, &oid, &selected(root, &oid, "large.txt")).unwrap();
    assert!(patch.contains("[Diff preview truncated at 512 KiB]"));
    assert!(patch.len() <= 512 * 1024 + 64);
    fs::write(root.join("large.txt"), large.replace("large", "staged")).unwrap();
    command(root, &["add", "large.txt"]);
    fs::write(root.join("large.txt"), large.replace("large", "unstaged")).unwrap();
    fs::write(root.join("untracked.txt"), &large).unwrap();
    let before = snapshot(root);
    for name in ["large.txt", "untracked.txt"] {
        let patch =
            git::file_details(root, WORKTREE_OID, &selected(root, WORKTREE_OID, name)).unwrap();
        assert!(patch.contains("[Diff preview truncated at 512 KiB]"));
        assert!(patch.len() <= 512 * 1024 + 64);
    }
    assert_eq!(snapshot(root), before);
}

#[test]
fn untracked_binary_and_directory_have_explicit_safe_previews() {
    let dir = init();
    let root = dir.path();
    fs::write(root.join("binary.dat"), b"not text\0secret binary data").unwrap();
    fs::write(root.join("empty.txt"), "").unwrap();
    fs::create_dir(root.join("directory")).unwrap();
    fs::write(root.join("directory/child.txt"), "child secret\n").unwrap();
    let before = snapshot(root);
    let binary = git::file_details(root, WORKTREE_OID, &file("binary.dat", "??")).unwrap();
    assert!(binary.contains("Binary file") && !binary.contains("secret binary data"));
    let directory = git::file_details(root, WORKTREE_OID, &file("directory", "??")).unwrap();
    assert!(directory.contains("Directory") && !directory.contains("child secret"));
    let empty = git::file_details(root, WORKTREE_OID, &file("empty.txt", "??")).unwrap();
    assert!(empty.contains("Untracked file contents"));
    assert_eq!(snapshot(root), before);
}

#[cfg(unix)]
#[test]
fn racing_untracked_replacements_never_follow_links_or_read_fifos() {
    use std::{
        ffi::CString,
        os::unix::{ffi::OsStrExt, fs::symlink},
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        thread,
    };

    let dir = init();
    let root = dir.path();
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("victim"), "outside secret contents\n").unwrap();
    fs::create_dir(root.join("parent")).unwrap();
    fs::write(root.join("parent/victim"), "inside contents\n").unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = Arc::clone(&stop);
    let root_path = root.to_owned();
    let outside_path = outside.path().to_owned();
    let writer = thread::spawn(move || {
        let parent = root_path.join("parent");
        let replacement = parent.join("replacement");
        let victim = parent.join("victim");
        while !thread_stop.load(Ordering::Relaxed) {
            fs::write(&replacement, "inside contents\n").unwrap();
            fs::rename(&replacement, &victim).unwrap();
            symlink(outside_path.join("victim"), &replacement).unwrap();
            fs::rename(&replacement, &victim).unwrap();
            let fifo = CString::new(replacement.as_os_str().as_bytes()).unwrap();
            assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
            fs::rename(&replacement, &victim).unwrap();
            // The reader may hold the original parent directory descriptor while
            // its pathname is temporarily replaced by an outside-directory link.
            fs::rename(&parent, root_path.join("held-parent")).unwrap();
            symlink(&outside_path, &parent).unwrap();
            fs::remove_file(&parent).unwrap();
            fs::rename(root_path.join("held-parent"), &parent).unwrap();
        }
    });
    let mut exposed = false;
    for _ in 0..300 {
        if let Ok(preview) = git::file_details(root, WORKTREE_OID, &file("parent/victim", "??")) {
            exposed |= preview.contains("outside secret contents");
        }
    }
    stop.store(true, Ordering::Relaxed);
    writer.join().unwrap();
    assert!(!exposed);
    assert_eq!(
        fs::read_to_string(outside.path().join("victim")).unwrap(),
        "outside secret contents\n"
    );
}

#[cfg(unix)]
#[test]
fn symlinks_do_not_expose_outside_contents_or_allow_parent_traversal() {
    use std::os::unix::fs::symlink;

    let dir = init();
    let root = dir.path();
    let outside = tempfile::tempdir().unwrap();
    fs::write(
        outside.path().join("secret.txt"),
        "outside secret contents\n",
    )
    .unwrap();
    symlink(outside.path().join("secret.txt"), root.join("link.txt")).unwrap();
    symlink(outside.path(), root.join("linked-directory")).unwrap();
    let before = snapshot(root);
    let patch = git::file_details(root, WORKTREE_OID, &file("link.txt", "??")).unwrap();
    assert!(patch.contains("Symbolic link") && patch.contains("not followed"));
    assert!(!patch.contains("outside secret contents"));
    for status in ["??", " M"] {
        assert!(
            git::file_details(
                root,
                WORKTREE_OID,
                &file("linked-directory/secret.txt", status)
            )
            .is_err()
        );
    }
    assert_eq!(snapshot(root), before);
    assert_eq!(
        fs::read_to_string(outside.path().join("secret.txt")).unwrap(),
        "outside secret contents\n"
    );
}
