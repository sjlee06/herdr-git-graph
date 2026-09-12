#![cfg(unix)]

use std::{fs, os::unix::fs::PermissionsExt, process::Command};

#[test]
fn launchers_preserve_repository_context_and_choose_the_correct_layout() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repo with spaces");
    fs::create_dir(&repo).unwrap();
    assert!(
        Command::new("git")
            .arg("init")
            .arg(&repo)
            .output()
            .unwrap()
            .status
            .success()
    );
    let repo = fs::canonicalize(repo).unwrap();
    let herdr = temp.path().join("fake herdr");
    let capture = temp.path().join("arguments");
    fs::write(
        &herdr,
        "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$HGG_CAPTURE\"\nexit \"${HGG_EXIT:-0}\"\n",
    )
    .unwrap();
    fs::set_permissions(&herdr, fs::Permissions::from_mode(0o755)).unwrap();
    for sidebar in [false, true] {
        let output = Command::new(env!("CARGO_BIN_EXE_herdr-git-graph"))
            .arg(if sidebar {
                "--open-sidebar"
            } else {
                "--open-pane"
            })
            .arg("--repo")
            .arg(&repo)
            .env("HERDR_BIN_PATH", &herdr)
            .env("HERDR_WORKSPACE_ID", "w-test")
            .env("HERDR_PANE_ID", "w-test:p-source")
            .env("HGG_CAPTURE", &capture)
            .env_remove("HGG_EXIT")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let args = fs::read_to_string(&capture).unwrap();
        let args: Vec<_> = args.lines().collect();
        let value = |flag| args.windows(2).find(|pair| pair[0] == flag).unwrap()[1];
        assert_eq!(value("--plugin"), "herdr.git-graph");
        assert_eq!(
            value("--entrypoint"),
            if sidebar { "sidebar" } else { "graph" }
        );
        assert_eq!(value("--placement"), if sidebar { "split" } else { "tab" });
        assert_eq!(value("--cwd"), repo.to_str().unwrap());
        assert_eq!(
            value("--env"),
            format!("HERDR_GIT_GRAPH_REPO={}", repo.display())
        );
        assert_eq!(value("--workspace"), "w-test");
        if sidebar {
            assert_eq!(value("--direction"), "right");
            assert_eq!(value("--target-pane"), "w-test:p-source");
            assert!(args.contains(&"--no-focus"));
            assert!(!args.contains(&"--focus"));
        } else {
            assert!(args.contains(&"--focus"));
            assert!(!args.contains(&"--direction"));
        }
    }
    let failed = Command::new(env!("CARGO_BIN_EXE_herdr-git-graph"))
        .args(["--open-sidebar", "--repo"])
        .arg(&repo)
        .env("HERDR_BIN_PATH", &herdr)
        .env("HGG_CAPTURE", &capture)
        .env("HGG_EXIT", "1")
        .output()
        .unwrap();
    assert!(!failed.status.success());
    assert!(String::from_utf8_lossy(&failed.stderr).contains("패널을 열지 못했습니다"));
}

#[test]
fn incompatible_launch_modes_are_rejected_before_opening_a_pane() {
    for flags in [
        ["--open-sidebar", "--open-pane"],
        ["--open-sidebar", "--sidebar"],
        ["--open-sidebar", "--demo"],
        ["--open-pane", "--sidebar"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_herdr-git-graph"))
            .args(flags)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
    }
}
