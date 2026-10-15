#![cfg(unix)]

use serde_json::{Value, json};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    process::{Command, Output, Stdio},
    sync::{Arc, Barrier},
    thread,
};

// Model Herdr independently of the launcher's persistence format. Each socket
// has its own live workspace/pane state, while all calls share a JSON call log.
const FAKE_HERDR: &str = r#"#!/usr/bin/env python3
import copy
import fcntl
import json
import os
import sys
import time

args = sys.argv[1:]
socket = os.environ['HERDR_SOCKET_PATH']
def flag(name, default=None):
    return args[args.index(name) + 1] if name in args else default

def live(pane):
    return {key: pane[key] for key in
            ('pane_id', 'terminal_id', 'workspace_id', 'tab_id', 'focused')}

with open(os.environ['HGG_SIMULATOR'] + '.lock', 'a') as lock:
    fcntl.flock(lock, fcntl.LOCK_EX)
    with open(os.environ['HGG_SIMULATOR']) as file:
        state = json.load(file)
    state['calls'].append({'socket': socket, 'args': args})
    session = state['sessions'].setdefault(socket, copy.deepcopy(state['seed']))
    panes = session['panes']
    result = {}
    code = 0
    if args[:3] == ['plugin', 'pane', 'open']:
        time.sleep(float(os.environ.get('HGG_OPEN_DELAY', '0')))
        if os.environ.get('HGG_EXIT') == '1':
            code = 1
        else:
            target = flag('--target-pane')
            source = next((p for p in panes if p['pane_id'] == target), None)
            workspace = source['workspace_id'] if source else flag('--workspace', 'w-test')
            session['next_id'] += 1
            number = str(session['next_id'])
            pane = {'pane_id': workspace + ':p-plugin-' + number,
                    'terminal_id': workspace + ':t-plugin-' + number,
                    'workspace_id': workspace,
                    'tab_id': source['tab_id'] if source else workspace + ':tab-plugin-' + number,
                    'focused': '--focus' in args,
                    'source_pane_id': target, 'ratio': 0.5,
                    'entrypoint': flag('--entrypoint')}
            panes.append(pane)
            result = {'plugin_pane': {'plugin_id': 'herdr.git-graph',
                      'entrypoint': flag('--entrypoint'), 'pane': live(pane)}}
    elif args[:2] == ['pane', 'list']:
        workspace = flag('--workspace')
        assert workspace is not None, args
        result = {'panes': [live(p) for p in panes if p['workspace_id'] == workspace]}
    elif args[:2] == ['workspace', 'get']:
        result = {'workspace': session['workspaces'][args[2]]}
    elif args[:2] == ['workspace', 'list']:
        result = {'workspaces': list(session['workspaces'].values())}
    elif args[:2] == ['tab', 'list']:
        workspace = flag('--workspace')
        result = {'tabs': [{'tab_id': tab, 'workspace_id': workspace}
                           for tab in session['tabs'] if tab.startswith(workspace + ':')]}
    elif args[:2] == ['pane', 'layout']:
        pane = next(p for p in panes if p['pane_id'] == flag('--pane'))
        total = session['tabs'][pane['tab_id']]['width']
        sidebar = next((p for p in reversed(panes) if p.get('source_pane_id') and
                        p['tab_id'] == pane['tab_id']), None)
        area = {'x': 0, 'y': 0, 'width': total, 'height': 30}
        if sidebar:
            first = round(total * sidebar['ratio'])
            layout = {'panes': [
                {'pane_id': sidebar['source_pane_id'],
                 'rect': {'x': 0, 'y': 0, 'width': first, 'height': 30}},
                {'pane_id': sidebar['pane_id'],
                 'rect': {'x': first, 'y': 0, 'width': total - first, 'height': 30}}],
                'splits': [{'direction': 'right', 'ratio': sidebar['ratio'], 'rect': area}]}
        else:
            layout = {'panes': [{'pane_id': pane['pane_id'], 'rect': area}], 'splits': []}
        result = {'layout': layout}
    elif args[:2] == ['pane', 'resize']:
        target = flag('--pane')
        sidebar = next(p for p in panes if p.get('source_pane_id') == target or
                       (p['pane_id'] == target and p.get('source_pane_id')))
        amount = float(flag('--amount'))
        sidebar['ratio'] += amount if flag('--direction') == 'right' else -amount
        result = {'pane': live(sidebar)}
    elif args[:2] == ['pane', 'close']:
        assert len(args) == 3, args
        pane = next(p for p in panes if p['pane_id'] == args[2])
        if os.environ.get('HGG_CLOSE_EXIT') == '1':
            code = 1
        else:
            panes.remove(pane)
            result = {'closed': True, 'pane_id': args[2]}
    elif args[:2] == ['pane', 'move']:
        assert '--no-focus' in args and '--focus' not in args, args
        assert flag('--split') == 'right', args
        pane = next(p for p in panes if p['pane_id'] == args[2])
        source = next(p for p in panes if p['pane_id'] == flag('--target-pane'))
        assert source['tab_id'] == flag('--tab'), args
        assert source['workspace_id'] == pane['workspace_id'], args
        ratio = float(flag('--ratio'))
        assert 0.1 <= ratio <= 0.9, args
        if (session['tabs'][pane['tab_id']].get('zoomed', False) or
                session['tabs'][source['tab_id']].get('zoomed', False)):
            result = {'move_result': {'changed': False, 'reason': 'zoomed_tab', 'pane': live(pane)}}
        else:
            pane.update(tab_id=source['tab_id'], source_pane_id=source['pane_id'], ratio=ratio)
            result = {'move_result': {'changed': True, 'reason': None, 'pane': live(pane)}}
    else:
        raise AssertionError('Unexpected Herdr command: ' + repr(args))
    with open(os.environ['HGG_SIMULATOR'], 'w') as file:
        json.dump(state, file)
    print(json.dumps({'result': result}))
    sys.exit(code)
"#;

struct Fixture {
    temp: tempfile::TempDir,
    repo: PathBuf,
    herdr: PathBuf,
    simulator: PathBuf,
    socket: PathBuf,
}

impl Fixture {
    fn new() -> Self {
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
        fs::write(&herdr, FAKE_HERDR).unwrap();
        fs::set_permissions(&herdr, fs::Permissions::from_mode(0o755)).unwrap();
        let simulator = temp.path().join("simulator.json");
        let socket = temp.path().join("herdr.sock");
        let mut seed = json!({"next_id": 0, "panes": [], "workspaces": {}, "tabs": {}});
        for workspace in ["w-test", "w-other", "w-action"] {
            seed["workspaces"][workspace] = json!({
                "workspace_id": workspace,
                "active_tab_id": format!("{workspace}:tab-a"),
                "focused": true
            });
            for (suffix, tab, width) in [("source", "a", 200), ("other", "b", 400)] {
                let tab_id = format!("{workspace}:tab-{tab}");
                seed["tabs"][&tab_id] = json!({"width": width});
                seed["panes"].as_array_mut().unwrap().push(json!({
                    "pane_id": format!("{workspace}:p-{suffix}"),
                    "terminal_id": format!("{workspace}:t-{suffix}"),
                    "workspace_id": workspace,
                    "tab_id": tab_id,
                    "focused": true
                }));
            }
        }
        let state =
            json!({"seed": seed, "sessions": {socket.to_str().unwrap(): seed}, "calls": []});
        fs::write(&simulator, serde_json::to_vec(&state).unwrap()).unwrap();
        Self {
            temp,
            repo,
            herdr,
            simulator,
            socket,
        }
    }

    fn command_without_repo(&self, mode: &str) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_herdr-git-graph"));
        command
            .arg(mode)
            .env("HERDR_BIN_PATH", &self.herdr)
            .env(
                "HERDR_PLUGIN_STATE_DIR",
                self.temp.path().join("plugin state"),
            )
            .env("HERDR_SOCKET_PATH", &self.socket)
            .env("HERDR_WORKSPACE_ID", "w-test")
            .env("HERDR_PANE_ID", "w-test:p-source")
            .env("HGG_SIMULATOR", &self.simulator)
            .env_remove("HERDR_PLUGIN_CONTEXT_JSON")
            .env_remove("HERDR_PLUGIN_EVENT")
            .env_remove("HERDR_PLUGIN_EVENT_JSON")
            .env_remove("HERDR_GIT_GRAPH_REPO")
            .env_remove("HERDR_GIT_GRAPH_THEME")
            .env_remove("HERDR_PLUGIN_ROOT")
            .env_remove("HGG_EXIT")
            .env_remove("HGG_CLOSE_EXIT")
            .env_remove("HGG_OPEN_DELAY");
        command
    }

    fn command(&self, mode: &str) -> Command {
        let mut command = self.command_without_repo(mode);
        if mode != "--sync-sidebar" {
            command.arg("--repo").arg(&self.repo);
        }
        command
    }

    fn run(&self, mode: &str) -> Value {
        successful(self.command(mode).output().unwrap())
    }

    fn state(&self) -> Value {
        serde_json::from_slice(&fs::read(&self.simulator).unwrap()).unwrap()
    }

    fn update_session(&self, update: impl FnOnce(&mut Value)) {
        let mut state = self.state();
        update(&mut state["sessions"][self.socket.to_str().unwrap()]);
        fs::write(&self.simulator, serde_json::to_vec(&state).unwrap()).unwrap();
    }

    fn calls(&self, prefix: &[&str]) -> Vec<Vec<String>> {
        self.state()["calls"]
            .as_array()
            .unwrap()
            .iter()
            .map(|call| {
                call["args"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|arg| arg.as_str().unwrap().to_owned())
                    .collect::<Vec<_>>()
            })
            .filter(|args| {
                args.iter()
                    .take(prefix.len())
                    .map(String::as_str)
                    .eq(prefix.iter().copied())
            })
            .collect()
    }

    fn sidebar(&self, id: &str) -> Value {
        self.state()["sessions"][self.socket.to_str().unwrap()]["panes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|pane| pane["pane_id"] == id)
            .unwrap()
            .clone()
    }
}

fn successful(output: Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    if output.stdout.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
            panic!(
                "expected one JSON response: {error}: {}",
                String::from_utf8_lossy(&output.stdout)
            )
        })
    }
}

fn pane(response: &Value) -> &Value {
    assert_eq!(
        response["result"]["plugin_pane"]["plugin_id"],
        "herdr.git-graph"
    );
    assert_eq!(response["result"]["plugin_pane"]["entrypoint"], "sidebar");
    &response["result"]["plugin_pane"]["pane"]
}

fn flag<'a>(args: &'a [String], name: &str) -> &'a str {
    args.windows(2).find(|pair| pair[0] == name).unwrap()[1].as_str()
}

#[test]
fn launchers_preserve_repository_context_and_choose_the_correct_layout() {
    let fixture = Fixture::new();
    for sidebar in [false, true] {
        fixture.run(if sidebar {
            "--open-sidebar"
        } else {
            "--open-pane"
        });
        let opens = fixture.calls(&["plugin", "pane", "open"]);
        let args = opens.last().unwrap();
        assert_eq!(flag(args, "--plugin"), "herdr.git-graph");
        assert_eq!(
            flag(args, "--entrypoint"),
            if sidebar { "sidebar" } else { "graph" }
        );
        assert_eq!(
            flag(args, "--placement"),
            if sidebar { "split" } else { "tab" }
        );
        assert_eq!(flag(args, "--cwd"), fixture.repo.to_str().unwrap());
        assert!(args.windows(2).any(|pair| pair[0] == "--env"
            && pair[1] == format!("HERDR_GIT_GRAPH_REPO={}", fixture.repo.display())));
        if sidebar {
            assert!(!args.iter().any(|arg| arg == "--workspace"));
            assert_eq!(flag(args, "--direction"), "right");
            assert_eq!(flag(args, "--target-pane"), "w-test:p-source");
            assert!(args.iter().any(|arg| arg == "--no-focus"));
            assert!(!args.iter().any(|arg| arg == "--focus"));
        } else {
            assert_eq!(flag(args, "--workspace"), "w-test");
            assert!(args.iter().any(|arg| arg == "--focus"));
            assert!(!args.iter().any(|arg| arg == "--direction"));
        }
    }
    let layouts = fixture.calls(&["pane", "layout"]);
    assert_eq!(layouts.len(), 2);
    let resizes = fixture.calls(&["pane", "resize"]);
    assert_eq!(resizes.len(), 1);
    assert_eq!(flag(&resizes[0], "--pane"), "w-test:p-source");
    assert_eq!(flag(&resizes[0], "--direction"), "right");
    assert!((flag(&resizes[0], "--amount").parse::<f64>().unwrap() - 0.3).abs() < 0.0001);
}

#[test]
fn sidebar_open_failures_do_not_leave_a_reusable_record() {
    let fixture = Fixture::new();
    let failed = fixture
        .command("--open-sidebar")
        .env("HGG_EXIT", "1")
        .output()
        .unwrap();
    assert!(!failed.status.success());
    assert!(String::from_utf8_lossy(&failed.stderr).contains("패널을 열지 못했습니다"));
    fixture.run("--open-sidebar");
    fixture.run("--open-sidebar");
    assert_eq!(fixture.calls(&["plugin", "pane", "open"]).len(), 2);
}

#[test]
fn sidebar_uses_invocation_context_even_without_or_with_stale_pane_environment() {
    for inherited_pane in [None, Some("w-old:p-stale")] {
        let fixture = Fixture::new();
        let mut command = fixture.command("--open-sidebar");
        command
            .env(
                "HERDR_PLUGIN_CONTEXT_JSON",
                r#"{"focused_pane_id":"w-action:p-source","workspace_id":"w-action"}"#,
            )
            .env_remove("HERDR_PANE_ID")
            .env_remove("HERDR_WORKSPACE_ID");
        if let Some(id) = inherited_pane {
            command
                .env("HERDR_PANE_ID", id)
                .env("HERDR_WORKSPACE_ID", "w-old");
        }
        let response = successful(command.output().unwrap());
        assert_eq!(pane(&response)["workspace_id"], "w-action");
        let opens = fixture.calls(&["plugin", "pane", "open"]);
        assert_eq!(flag(&opens[0], "--target-pane"), "w-action:p-source");
        assert!(!opens[0].iter().any(|arg| arg == "--workspace"));
        let repeated = successful(command.output().unwrap());
        assert_eq!(pane(&response), pane(&repeated));
        assert_eq!(fixture.calls(&["plugin", "pane", "open"]).len(), 1);
        assert!(
            fixture
                .calls(&["pane", "list"])
                .iter()
                .all(|args| flag(args, "--workspace") == "w-action")
        );
    }
}

#[test]
fn sidebar_requires_a_nonempty_target_before_launching_herdr() {
    let fixture = Fixture::new();
    for context in ["{}", "invalid json", r#"{"focused_pane_id":""}"#] {
        let output = fixture
            .command("--open-sidebar")
            .env("HERDR_PLUGIN_CONTEXT_JSON", context)
            .env("HERDR_PANE_ID", "")
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("대상 패널이 없습니다"));
        assert!(fixture.calls(&[]).is_empty());
    }
}

#[test]
fn incompatible_launch_modes_are_rejected_before_opening_a_pane() {
    let fixture = Fixture::new();
    for flags in [
        ["--open-sidebar", "--open-pane"],
        ["--open-sidebar", "--sidebar"],
        ["--open-sidebar", "--demo"],
        ["--open-pane", "--sidebar"],
        ["--sync-sidebar", "--open-sidebar"],
        ["--sync-sidebar", "--open-pane"],
        ["--sync-sidebar", "--sidebar"],
        ["--sync-sidebar", "--demo"],
        ["--toggle-sidebar", "--open-sidebar"],
        ["--toggle-sidebar", "--open-pane"],
        ["--toggle-sidebar", "--sync-sidebar"],
        ["--toggle-sidebar", "--sidebar"],
        ["--toggle-sidebar", "--demo"],
        ["--toggle-sidebar", "--check"],
    ] {
        let output = fixture.command(flags[0]).arg(flags[1]).output().unwrap();
        assert_eq!(output.status.code(), Some(2));
    }
    assert!(fixture.calls(&[]).is_empty());
}

#[test]
fn theme_environment_and_cli_override_are_forwarded_to_the_new_pane() {
    let fixture = Fixture::new();
    for (flags, environment, expected) in [
        (vec![], "auto", "auto"),
        (vec![], "terminal", "terminal"),
        (vec!["--theme", "classic"], "terminal", "classic"),
    ] {
        successful(
            fixture
                .command("--open-pane")
                .args(flags)
                .env("HERDR_GIT_GRAPH_THEME", environment)
                .output()
                .unwrap(),
        );
        let opens = fixture.calls(&["plugin", "pane", "open"]);
        assert!(opens.last().unwrap().windows(2).any(
            |pair| pair[0] == "--env" && pair[1] == format!("HERDR_GIT_GRAPH_THEME={expected}")
        ));
    }
}

#[test]
fn rapid_repeated_opens_reuse_the_live_sidebar_without_resizing_it() {
    let fixture = Fixture::new();
    let first = fixture.run("--open-sidebar");
    let id = pane(&first)["pane_id"].as_str().unwrap();
    fixture.update_session(|session| {
        let saved = session["panes"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|p| p["pane_id"] == id)
            .unwrap();
        saved["focused"] = json!(true);
        saved["ratio"] = json!(0.7); // A manual resize must survive repeated opens.
    });
    for _ in 0..8 {
        let repeated = fixture.run("--open-sidebar");
        assert_eq!(pane(&repeated)["pane_id"], pane(&first)["pane_id"]);
        assert_eq!(pane(&repeated)["terminal_id"], pane(&first)["terminal_id"]);
        assert_eq!(
            pane(&repeated)["focused"],
            true,
            "reuse must return current live pane info"
        );
    }
    assert_eq!(fixture.calls(&["plugin", "pane", "open"]).len(), 1);
    assert_eq!(fixture.calls(&["pane", "resize"]).len(), 1);
    assert!(fixture.calls(&["pane", "move"]).is_empty());
    assert!(fixture.calls(&["pane", "list"]).len() >= 8);
    assert_eq!(fixture.sidebar(id)["ratio"], 0.7);
}

#[test]
fn sidebar_toggle_opens_closes_and_reopens_without_hook_resurrection() {
    let fixture = Fixture::new();
    let first = fixture.run("--toggle-sidebar");
    let id = pane(&first)["pane_id"].as_str().unwrap();
    assert_eq!(pane(&fixture.run("--open-sidebar")), pane(&first));
    // Closing must also work from the sidebar itself and outside a Git cwd.
    successful(
        fixture
            .command_without_repo("--toggle-sidebar")
            .env("HERDR_PANE_ID", id)
            .current_dir(fixture.temp.path())
            .output()
            .unwrap(),
    );
    assert_eq!(
        fixture.calls(&["pane", "close"]),
        vec![vec!["pane", "close", id]]
    );
    fixture.update_session(|session| {
        session["tabs"]
            .as_object_mut()
            .unwrap()
            .remove("w-test:tab-a");
        session["panes"]
            .as_array_mut()
            .unwrap()
            .retain(|p| p["tab_id"] != "w-test:tab-a");
        session["workspaces"]["w-test"]["active_tab_id"] = json!("w-test:tab-b");
    });
    fixture.run("--sync-sidebar");
    assert_eq!(fixture.calls(&["plugin", "pane", "open"]).len(), 1);
    assert!(fixture.calls(&["pane", "move"]).is_empty());
    let reopened = successful(
        fixture
            .command("--toggle-sidebar")
            .env("HERDR_PANE_ID", "w-test:p-other")
            .output()
            .unwrap(),
    );
    assert_ne!(pane(&reopened)["pane_id"], pane(&first)["pane_id"]);
    assert_eq!(pane(&reopened)["tab_id"], "w-test:tab-b");
}

#[test]
fn sidebar_toggle_close_failure_keeps_the_saved_sidebar() {
    let fixture = Fixture::new();
    let first = fixture.run("--open-sidebar");
    let failed = fixture
        .command("--toggle-sidebar")
        .env("HGG_CLOSE_EXIT", "1")
        .output()
        .unwrap();
    assert!(!failed.status.success());
    assert_eq!(pane(&fixture.run("--open-sidebar")), pane(&first));
    fixture.run("--toggle-sidebar");
    fixture.run("--sync-sidebar");
    assert_eq!(fixture.calls(&["plugin", "pane", "open"]).len(), 1);
    assert_eq!(fixture.calls(&["pane", "close"]).len(), 2);
}

#[test]
fn sidebar_toggle_never_closes_a_reused_terminal_or_another_workspace() {
    let fixture = Fixture::new();
    let first = fixture.run("--open-sidebar");
    let id = pane(&first)["pane_id"].as_str().unwrap();
    fixture.update_session(|session| {
        let reused = session["panes"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|p| p["pane_id"] == id)
            .unwrap();
        reused["terminal_id"] = json!("unrelated-terminal");
    });
    let replacement = fixture.run("--toggle-sidebar");
    assert_ne!(pane(&replacement)["pane_id"], pane(&first)["pane_id"]);
    assert!(fixture.calls(&["pane", "close"]).is_empty());
    let other = successful(
        fixture
            .command("--toggle-sidebar")
            .env("HERDR_WORKSPACE_ID", "w-other")
            .env("HERDR_PANE_ID", "w-other:p-source")
            .output()
            .unwrap(),
    );
    fixture.run("--toggle-sidebar");
    assert_eq!(fixture.sidebar(id)["terminal_id"], "unrelated-terminal");
    assert_eq!(
        fixture.sidebar(pane(&other)["pane_id"].as_str().unwrap())["terminal_id"],
        pane(&other)["terminal_id"]
    );
    assert_eq!(
        fixture.calls(&["pane", "close"]),
        vec![vec![
            "pane",
            "close",
            pane(&replacement)["pane_id"].as_str().unwrap()
        ]]
    );
}

#[test]
fn explicit_open_moves_the_existing_sidebar_to_the_invocation_tab() {
    let fixture = Fixture::new();
    let first = fixture.run("--open-sidebar");
    let moved = successful(
        fixture
            .command("--open-sidebar")
            .env("HERDR_PANE_ID", "w-test:p-other")
            .output()
            .unwrap(),
    );
    assert_eq!(pane(&moved)["pane_id"], pane(&first)["pane_id"]);
    assert_eq!(pane(&moved)["terminal_id"], pane(&first)["terminal_id"]);
    assert_eq!(pane(&moved)["tab_id"], "w-test:tab-b");
    assert_eq!(pane(&moved)["focused"], false);
    assert_eq!(fixture.calls(&["plugin", "pane", "open"]).len(), 1);
    let moves = fixture.calls(&["pane", "move"]);
    assert_eq!(moves.len(), 1);
    assert_eq!(moves[0][2], pane(&first)["pane_id"].as_str().unwrap());
    assert_eq!(flag(&moves[0], "--tab"), "w-test:tab-b");
    assert_eq!(flag(&moves[0], "--target-pane"), "w-test:p-other");
    assert!((flag(&moves[0], "--ratio").parse::<f64>().unwrap() - 0.9).abs() < 0.0001);
}

#[test]
fn sidebars_are_isolated_by_workspace_and_socket_session() {
    let fixture = Fixture::new();
    let first = fixture.run("--open-sidebar");
    let other_workspace = successful(
        fixture
            .command("--open-sidebar")
            .env("HERDR_WORKSPACE_ID", "w-other")
            .env("HERDR_PANE_ID", "w-other:p-source")
            .output()
            .unwrap(),
    );
    assert_eq!(pane(&other_workspace)["workspace_id"], "w-other");
    assert_ne!(pane(&first)["pane_id"], pane(&other_workspace)["pane_id"]);
    let other_socket = fixture.temp.path().join("second herdr.sock");
    let other_session = successful(
        fixture
            .command("--open-sidebar")
            .env("HERDR_SOCKET_PATH", &other_socket)
            .output()
            .unwrap(),
    );
    // Herdr sessions deliberately reuse pane IDs, so only socket scoping can
    // distinguish these two otherwise identical live records.
    assert_eq!(pane(&first)["pane_id"], pane(&other_session)["pane_id"]);
    assert_eq!(fixture.calls(&["plugin", "pane", "open"]).len(), 3);
    assert_eq!(pane(&fixture.run("--open-sidebar")), pane(&first));
    assert_eq!(
        pane(&successful(
            fixture
                .command("--open-sidebar")
                .env("HERDR_SOCKET_PATH", &other_socket)
                .output()
                .unwrap()
        )),
        pane(&other_session)
    );
    assert_eq!(
        pane(&successful(
            fixture
                .command("--open-sidebar")
                .env("HERDR_WORKSPACE_ID", "w-other")
                .env("HERDR_PANE_ID", "w-other:p-source")
                .output()
                .unwrap()
        )),
        pane(&other_workspace)
    );
    assert_eq!(fixture.calls(&["plugin", "pane", "open"]).len(), 3);
}

#[test]
fn sync_without_a_saved_sidebar_is_a_noop_before_repository_discovery() {
    let fixture = Fixture::new();
    successful(
        fixture
            .command("--sync-sidebar")
            .current_dir(fixture.temp.path())
            .env("HERDR_GIT_GRAPH_REPO", "/missing/repository")
            .env_remove("HERDR_PANE_ID")
            .output()
            .unwrap(),
    );
    assert!(fixture.calls(&[]).is_empty());
}

#[test]
fn sync_follows_the_latest_active_tab_and_preserves_user_resized_width() {
    let fixture = Fixture::new();
    let first = fixture.run("--open-sidebar");
    let id = pane(&first)["pane_id"].as_str().unwrap();
    fixture.update_session(|session| {
        session["workspaces"]["w-test"]["active_tab_id"] = json!("w-test:tab-b");
        session["panes"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|p| p["pane_id"] == id)
            .unwrap()["ratio"] = json!(0.7);
    });
    successful(fixture.command("--sync-sidebar")
        .current_dir(fixture.temp.path()).env("HERDR_GIT_GRAPH_REPO", "/missing/repository")
        .env("HERDR_WORKSPACE_ID", "w-old").env("HERDR_PANE_ID", "w-old:p-stale")
        .env("HERDR_PLUGIN_CONTEXT_JSON", r#"{"workspace_id":"w-test","active_tab_id":"w-test:tab-a","focused_pane_id":"w-test:p-source"}"#)
        .output().unwrap());
    assert_eq!(fixture.sidebar(id)["tab_id"], "w-test:tab-b");
    assert_eq!(
        fixture.sidebar(id)["terminal_id"],
        pane(&first)["terminal_id"]
    );
    assert_eq!(fixture.sidebar(id)["focused"], false);
    let moves = fixture.calls(&["pane", "move"]);
    assert_eq!(moves.len(), 1);
    assert_eq!(flag(&moves[0], "--target-pane"), "w-test:p-other");
    assert_eq!(flag(&moves[0], "--tab"), "w-test:tab-b");
    assert!((flag(&moves[0], "--ratio").parse::<f64>().unwrap() - 0.85).abs() < 0.0001);
    fixture.run("--sync-sidebar");
    assert_eq!(fixture.calls(&["pane", "move"]).len(), 1);
    assert_eq!(fixture.calls(&["plugin", "pane", "open"]).len(), 1);
}

#[test]
fn sync_does_not_move_a_sidebar_in_an_unfocused_workspace() {
    let fixture = Fixture::new();
    let first = fixture.run("--open-sidebar");
    fixture.update_session(|session| {
        session["workspaces"]["w-test"]["active_tab_id"] = json!("w-test:tab-b");
        session["workspaces"]["w-test"]["focused"] = json!(false);
    });
    fixture.run("--sync-sidebar");
    assert_eq!(
        fixture.sidebar(pane(&first)["pane_id"].as_str().unwrap())["tab_id"],
        "w-test:tab-a"
    );
    assert!(fixture.calls(&["pane", "move"]).is_empty());
    assert_eq!(fixture.calls(&["plugin", "pane", "open"]).len(), 1);
}

#[test]
fn sync_clamps_preserved_width_for_narrow_and_wide_destinations() {
    for (width, expected_ratio) in [(40, 0.1), (80, 0.25), (1000, 0.9)] {
        let fixture = Fixture::new();
        let first = fixture.run("--open-sidebar");
        let id = pane(&first)["pane_id"].as_str().unwrap();
        fixture.update_session(|session| {
            session["workspaces"]["w-test"]["active_tab_id"] = json!("w-test:tab-b");
            session["tabs"]["w-test:tab-b"]["width"] = json!(width);
            session["panes"]
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .find(|p| p["pane_id"] == id)
                .unwrap()["ratio"] = json!(0.7);
        });
        fixture.run("--sync-sidebar");
        let moves = fixture.calls(&["pane", "move"]);
        assert_eq!(moves.len(), 1);
        assert!(
            (flag(&moves[0], "--ratio").parse::<f64>().unwrap() - expected_ratio).abs() < 0.0001
        );
    }
}

#[test]
fn constrained_moves_do_not_overwrite_the_users_preferred_width() {
    let fixture = Fixture::new();
    let first = fixture.run("--open-sidebar");
    let id = pane(&first)["pane_id"].as_str().unwrap();
    fixture.update_session(|session| {
        session["workspaces"]["w-test"]["active_tab_id"] = json!("w-test:tab-b");
        session["tabs"]["w-test:tab-b"]["width"] = json!(40);
        session["panes"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|pane| pane["pane_id"] == id)
            .unwrap()["ratio"] = json!(0.7); // User asks for 60 columns.
    });
    fixture.run("--sync-sidebar");
    assert!((fixture.sidebar(id)["ratio"].as_f64().unwrap() - 0.1).abs() < 0.0001);
    fixture.update_session(|session| {
        session["workspaces"]["w-test"]["active_tab_id"] = json!("w-test:tab-a");
    });
    fixture.run("--sync-sidebar");
    assert!(
        (fixture.sidebar(id)["ratio"].as_f64().unwrap() - 0.7).abs() < 0.0001,
        "back on a wide tab, the sidebar must regain 60 columns, not keep the clamped 36"
    );
}

#[test]
fn closed_or_terminal_reused_sidebar_records_are_cleared_without_respawning() {
    for terminal_reused in [false, true] {
        let fixture = Fixture::new();
        let first = fixture.run("--open-sidebar");
        let id = pane(&first)["pane_id"].as_str().unwrap();
        fixture.update_session(|session| {
            session["workspaces"]["w-test"]["active_tab_id"] = json!("w-test:tab-b");
            let panes = session["panes"].as_array_mut().unwrap();
            if terminal_reused {
                panes.iter_mut().find(|p| p["pane_id"] == id).unwrap()["terminal_id"] =
                    json!("unrelated-terminal");
            } else {
                panes.retain(|p| p["pane_id"] != id);
            }
        });
        fixture.run("--sync-sidebar");
        assert!(fixture.calls(&["pane", "move"]).is_empty());
        assert_eq!(fixture.calls(&["plugin", "pane", "open"]).len(), 1);
        let call_count = fixture.calls(&[]).len();
        fixture.run("--sync-sidebar");
        assert_eq!(
            fixture.calls(&[]).len(),
            call_count,
            "invalid record must be cleared, not rechecked forever"
        );
        let replacement = fixture.run("--open-sidebar");
        assert_ne!(pane(&replacement)["pane_id"], pane(&first)["pane_id"]);
        assert_ne!(
            pane(&replacement)["terminal_id"],
            pane(&first)["terminal_id"]
        );
        assert_eq!(fixture.calls(&["plugin", "pane", "open"]).len(), 2);
    }
}

#[test]
fn sync_recovers_after_containing_tab_closes_without_rediscovering_a_repository() {
    let fixture = Fixture::new();
    let first = successful(
        fixture
            .command("--open-sidebar")
            .args(["--theme", "classic"])
            .output()
            .unwrap(),
    );
    let id = pane(&first)["pane_id"].as_str().unwrap();
    fixture.update_session(|session| {
        session["workspaces"]["w-test"]["active_tab_id"] = json!("w-test:tab-b");
        session["panes"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|pane| pane["pane_id"] == id)
            .unwrap()["ratio"] = json!(0.7);
    });
    fixture.run("--sync-sidebar");
    assert_eq!(fixture.sidebar(id)["tab_id"], "w-test:tab-b");
    fixture.update_session(|session| {
        session["panes"]
            .as_array_mut()
            .unwrap()
            .retain(|pane| pane["tab_id"] != "w-test:tab-b");
        session["tabs"]
            .as_object_mut()
            .unwrap()
            .remove("w-test:tab-b");
        session["workspaces"]["w-test"]["active_tab_id"] = json!("w-test:tab-a");
    });
    // The close event's context still points to the now-deleted tab/pane.
    successful(fixture.command("--sync-sidebar")
        .current_dir(fixture.temp.path())
        .env("HERDR_GIT_GRAPH_REPO", "/missing/repository")
        .env("HERDR_PLUGIN_EVENT", "tab.closed")
        .env("HERDR_PLUGIN_EVENT_JSON", r#"{"event":"tab.closed","data":{"type":"tab_closed","workspace_id":"w-test","tab_id":"w-test:tab-b"}}"#)
        .env("HERDR_PLUGIN_CONTEXT_JSON", r#"{"workspace_id":"w-test","focused_pane_id":"w-test:p-other"}"#)
        .output().unwrap());
    let opens = fixture.calls(&["plugin", "pane", "open"]);
    assert_eq!(opens.len(), 2);
    assert_eq!(flag(&opens[1], "--target-pane"), "w-test:p-source");
    assert_eq!(flag(&opens[1], "--cwd"), fixture.repo.to_str().unwrap());
    assert!(
        opens[1]
            .windows(2)
            .any(|pair| pair == ["--env", "HERDR_GIT_GRAPH_THEME=classic"])
    );
    fixture.run("--sync-sidebar");
    assert_eq!(fixture.calls(&["plugin", "pane", "open"]).len(), 2);
    let reused = fixture.run("--open-sidebar");
    assert_ne!(pane(&reused)["terminal_id"], pane(&first)["terminal_id"]);
    assert_eq!(pane(&reused)["tab_id"], "w-test:tab-a");
    assert!(
        (fixture.sidebar(pane(&reused)["pane_id"].as_str().unwrap())["ratio"]
            .as_f64()
            .unwrap()
            - 0.7)
            .abs()
            < 0.0001,
        "a replacement must restore the preferred 60-column width"
    );
}

#[test]
fn direct_sidebar_close_opts_out_even_if_its_tab_is_closed_before_the_hook_runs() {
    for event in ["pane.closed", "pane.exited"] {
        let fixture = Fixture::new();
        let first = fixture.run("--open-sidebar");
        fixture.update_session(|session| {
            session["panes"]
                .as_array_mut()
                .unwrap()
                .retain(|pane| pane["tab_id"] != "w-test:tab-a");
            session["tabs"]
                .as_object_mut()
                .unwrap()
                .remove("w-test:tab-a");
            session["workspaces"]["w-test"]["active_tab_id"] = json!("w-test:tab-b");
        });
        successful(
            fixture
                .command("--sync-sidebar")
                .env("HERDR_PLUGIN_EVENT", event)
                .env(
                    "HERDR_PLUGIN_EVENT_JSON",
                    json!({"event": event, "data": {
                        "workspace_id": "w-test", "pane_id": pane(&first)["pane_id"]
                    }})
                    .to_string(),
                )
                .output()
                .unwrap(),
        );
        fixture.run("--sync-sidebar");
        assert_eq!(fixture.calls(&["plugin", "pane", "open"]).len(), 1);
    }
}

#[test]
fn closing_the_last_tab_does_not_attempt_to_reopen_a_sidebar_in_another_workspace() {
    let fixture = Fixture::new();
    fixture.run("--open-sidebar");
    fixture.update_session(|session| {
        session["workspaces"]
            .as_object_mut()
            .unwrap()
            .remove("w-test");
        session["panes"]
            .as_array_mut()
            .unwrap()
            .retain(|pane| pane["workspace_id"] != "w-test");
    });
    fixture.run("--sync-sidebar");
    assert_eq!(fixture.calls(&["plugin", "pane", "open"]).len(), 1);
    let calls = fixture.calls(&[]).len();
    fixture.run("--sync-sidebar");
    assert_eq!(
        fixture.calls(&[]).len(),
        calls,
        "removed workspace must clear its record"
    );
}

#[test]
fn concurrent_close_and_focus_hooks_restore_exactly_one_sidebar() {
    let fixture = Fixture::new();
    let first = fixture.run("--open-sidebar");
    fixture.update_session(|session| {
        session["panes"]
            .as_array_mut()
            .unwrap()
            .retain(|pane| pane["tab_id"] != "w-test:tab-a");
        session["tabs"]
            .as_object_mut()
            .unwrap()
            .remove("w-test:tab-a");
        session["workspaces"]["w-test"]["active_tab_id"] = json!("w-test:tab-b");
    });
    let barrier = Arc::new(Barrier::new(6));
    let threads: Vec<_> = (0..6)
        .map(|index| {
            let mut command = fixture.command("--sync-sidebar");
            command.env("HGG_OPEN_DELAY", "0.1").env(
                "HERDR_PLUGIN_EVENT",
                if index % 2 == 0 {
                    "tab.closed"
                } else {
                    "tab.focused"
                },
            );
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                successful(command.output().unwrap());
            })
        })
        .collect();
    for thread in threads {
        thread.join().unwrap();
    }
    assert_eq!(fixture.calls(&["plugin", "pane", "open"]).len(), 2);
    let replacement = successful(
        fixture
            .command("--open-sidebar")
            .env("HERDR_PANE_ID", "w-test:p-other")
            .output()
            .unwrap(),
    );
    // An unrelated or delayed exit event must not discard the replacement.
    successful(
        fixture
            .command("--sync-sidebar")
            .env("HERDR_PLUGIN_EVENT", "pane.exited")
            .env("HERDR_PLUGIN_CONTEXT_JSON", r#"{"workspace_id":"w-other"}"#)
            .env(
                "HERDR_PLUGIN_EVENT_JSON",
                json!({"data": {
                    "workspace_id": "w-test", "pane_id": pane(&first)["pane_id"]
                }})
                .to_string(),
            )
            .output()
            .unwrap(),
    );
    let reused = successful(
        fixture
            .command("--open-sidebar")
            .env("HERDR_PANE_ID", "w-test:p-other")
            .output()
            .unwrap(),
    );
    assert_eq!(pane(&replacement), pane(&reused));
    assert_eq!(fixture.calls(&["plugin", "pane", "open"]).len(), 2);
}

#[test]
fn explicit_open_recreates_a_closed_or_terminal_reused_sidebar() {
    for terminal_reused in [false, true] {
        let fixture = Fixture::new();
        let first = fixture.run("--open-sidebar");
        let id = pane(&first)["pane_id"].as_str().unwrap();
        fixture.update_session(|session| {
            let panes = session["panes"].as_array_mut().unwrap();
            if terminal_reused {
                panes.iter_mut().find(|p| p["pane_id"] == id).unwrap()["terminal_id"] =
                    json!("unrelated-terminal");
            } else {
                panes.retain(|p| p["pane_id"] != id);
            }
        });
        let replacement = fixture.run("--open-sidebar");
        assert_ne!(pane(&replacement)["pane_id"], pane(&first)["pane_id"]);
        assert_eq!(pane(&fixture.run("--open-sidebar")), pane(&replacement));
        assert_eq!(fixture.calls(&["plugin", "pane", "open"]).len(), 2);
        assert!(fixture.calls(&["pane", "move"]).is_empty());
    }
}

#[test]
fn saved_sidebar_reuse_and_moves_do_not_require_repository_discovery() {
    let fixture = Fixture::new();
    let first = fixture.run("--open-sidebar");
    assert!(
        !Command::new("git")
            .args(["rev-parse", "--show-toplevel"])
            .current_dir(fixture.temp.path())
            .output()
            .unwrap()
            .status
            .success(),
        "this regression must run outside a Git repository"
    );
    let mut command = fixture.command_without_repo("--open-sidebar");
    command.current_dir(fixture.temp.path());
    let reused = successful(command.output().unwrap());
    assert_eq!(pane(&reused), pane(&first));
    let moved = successful(
        command
            .env("HERDR_PANE_ID", "w-test:p-other")
            .output()
            .unwrap(),
    );
    assert_eq!(pane(&moved)["pane_id"], pane(&first)["pane_id"]);
    assert_eq!(pane(&moved)["terminal_id"], pane(&first)["terminal_id"]);
    assert_eq!(pane(&moved)["tab_id"], "w-test:tab-b");
    assert_eq!(fixture.calls(&["plugin", "pane", "open"]).len(), 1);
    assert_eq!(fixture.calls(&["pane", "move"]).len(), 1);
}

#[test]
fn zoom_declined_sidebar_moves_fail_without_losing_the_saved_pane() {
    for mode in ["--open-sidebar", "--sync-sidebar"] {
        for zoomed_tab in ["w-test:tab-a", "w-test:tab-b"] {
            let fixture = Fixture::new();
            let first = fixture.run("--open-sidebar");
            let id = pane(&first)["pane_id"].as_str().unwrap();
            fixture.update_session(|session| {
                session["workspaces"]["w-test"]["active_tab_id"] = json!("w-test:tab-b");
                session["tabs"][zoomed_tab]["zoomed"] = json!(true);
            });
            let unchanged = fixture.sidebar(id);
            let mut command = fixture.command(mode);
            command.env("HERDR_PANE_ID", "w-test:p-other");
            let declined = command.output().unwrap();
            assert!(
                !declined.status.success(),
                "{mode} must not report an unchanged move as success"
            );
            assert!(
                declined.stdout.is_empty(),
                "a declined move must not emit a successful pane response"
            );
            assert!(
                String::from_utf8_lossy(&declined.stderr).contains("확대"),
                "the error must explain that pane zoom needs to be released: {}",
                String::from_utf8_lossy(&declined.stderr)
            );
            assert_eq!(fixture.sidebar(id), unchanged);
            assert_eq!(
                fixture.state()["sessions"][fixture.socket.to_str().unwrap()]["tabs"][zoomed_tab]["zoomed"],
                true
            );
            assert_eq!(fixture.calls(&["plugin", "pane", "open"]).len(), 1);
            assert_eq!(fixture.calls(&["pane", "move"]).len(), 1);

            fixture.update_session(|session| session["tabs"][zoomed_tab]["zoomed"] = json!(false));
            let retried = successful(command.output().unwrap());
            if mode == "--open-sidebar" {
                assert_eq!(pane(&retried)["pane_id"], pane(&first)["pane_id"]);
                assert_eq!(pane(&retried)["terminal_id"], pane(&first)["terminal_id"]);
                assert_eq!(pane(&retried)["tab_id"], "w-test:tab-b");
            }
            assert_eq!(fixture.sidebar(id)["tab_id"], "w-test:tab-b");
            assert_eq!(
                fixture.sidebar(id)["terminal_id"],
                pane(&first)["terminal_id"]
            );
            assert_eq!(fixture.sidebar(id)["focused"], false);
            assert_eq!(fixture.calls(&["plugin", "pane", "open"]).len(), 1);
            assert_eq!(fixture.calls(&["pane", "move"]).len(), 2);
        }
    }
}

#[test]
fn concurrent_sidebar_opens_create_exactly_one_pane() {
    let fixture = Fixture::new();
    let barrier = Arc::new(Barrier::new(6));
    let threads: Vec<_> = (0..6)
        .map(|_| {
            let mut command = fixture.command("--open-sidebar");
            command
                .env("HGG_OPEN_DELAY", "0.15")
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                successful(command.output().unwrap())
            })
        })
        .collect();
    let responses: Vec<_> = threads
        .into_iter()
        .map(|thread| thread.join().unwrap())
        .collect();
    for response in &responses[1..] {
        assert_eq!(pane(response), pane(&responses[0]));
    }
    assert_eq!(fixture.calls(&["plugin", "pane", "open"]).len(), 1);
    assert_eq!(fixture.calls(&["pane", "resize"]).len(), 1);
    assert!(fixture.calls(&["pane", "move"]).is_empty());
}
