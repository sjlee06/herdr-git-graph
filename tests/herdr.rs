#![cfg(unix)]

use serde_json::{Value, json};
use std::{
    fs,
    io::{BufRead, BufReader},
    os::{fd::AsRawFd, unix::fs::PermissionsExt},
    path::PathBuf,
    process::{Child, Command, Output, Stdio},
    sync::{Arc, Barrier},
    thread,
};

// The CLI and optional socket server share this small tree model and the same
// simulator file lock. Complex tree/process preservation belongs to the live
// test; this model covers replacement beside a terminal-reused former sidebar.
const FAKE_LAYOUT: &str = r#"
import copy


def live(pane):
    return {key: pane[key] for key in
            ('pane_id', 'terminal_id', 'workspace_id', 'tab_id', 'focused')}


def leaf(pane_id):
    return {'type': 'pane', 'pane_id': pane_id}


def ids(node):
    if node is None:
        return []
    if node['type'] == 'pane':
        return [node['pane_id']]
    return ids(node['first']) + ids(node['second'])


def remove(node, pane_id):
    if node is None or node['type'] == 'pane':
        return None if node and node['pane_id'] == pane_id else node
    first = remove(node['first'], pane_id)
    second = remove(node['second'], pane_id)
    if first is None or second is None:
        return first or second
    return dict(node, first=first, second=second)


def insert(node, target, pane_id, direction, ratio):
    if node['type'] == 'pane':
        if node['pane_id'] != target:
            return node
        return {'type': 'split', 'direction': direction, 'ratio': ratio,
                'first': node, 'second': leaf(pane_id)}
    return dict(node, first=insert(node['first'], target, pane_id, direction, ratio),
                second=insert(node['second'], target, pane_id, direction, ratio))


def root_for(session, tab_id):
    tab = session['tabs'][tab_id]
    members = [p for p in session['panes'] if p['tab_id'] == tab_id]
    root = tab.get('root')
    for pane_id in ids(root):
        if not any(p['pane_id'] == pane_id for p in members):
            root = remove(root, pane_id)
    for pane in members:
        if pane['pane_id'] not in ids(root):
            target = pane.get('source_pane_id')
            root = (insert(root, target if target in ids(root) else ids(root)[0],
                           pane['pane_id'], pane.get('direction', 'right'),
                           pane.get('ratio', 0.5)) if root else leaf(pane['pane_id']))

    def sync_ratios(node):
        # Existing tests simulate a user resize by editing pane['ratio'].
        if node is None or node['type'] == 'pane':
            return
        if node['second']['type'] == 'pane':
            pane = next(p for p in members if p['pane_id'] == node['second']['pane_id'])
            if pane.get('source_pane_id') in ids(node['first']):
                node['ratio'] = pane.get('ratio', node['ratio'])
        sync_ratios(node['first'])
        sync_ratios(node['second'])

    sync_ratios(root)
    tab['root'] = root
    return root


def layout(session, pane_id):
    pane = next(p for p in session['panes'] if p['pane_id'] == pane_id)
    tab = session['tabs'][pane['tab_id']]
    area = {'x': 0, 'y': 0, 'width': tab['width'], 'height': 30}
    members = [p for p in session['panes'] if p['tab_id'] == pane['tab_id']]
    focused = next((p['pane_id'] for p in members if p['focused']), pane_id)
    result = {'workspace_id': pane['workspace_id'], 'tab_id': pane['tab_id'],
              'area': area, 'focused_pane_id': focused,
              'zoomed': tab.get('zoomed', False), 'panes': [], 'splits': []}

    def visit(node, rect, path):
        if node['type'] == 'pane':
            result['panes'].append({'pane_id': node['pane_id'], 'rect': rect,
                                   'focused': node['pane_id'] == focused})
            return
        result['splits'].append({'id': 'split-' + path, 'direction': node['direction'],
                                 'ratio': node['ratio'], 'rect': rect.copy()})
        axis = 'width' if node['direction'] == 'right' else 'height'
        position = 'x' if node['direction'] == 'right' else 'y'
        amount = round(rect[axis] * node['ratio'])
        first, second = rect.copy(), rect.copy()
        first[axis] = amount
        second[axis] -= amount
        second[position] += amount
        visit(node['first'], first, path + '0')
        visit(node['second'], second, path + '1')

    visit(root_for(session, pane['tab_id']), area.copy(), '')
    return result


def export(session, params):
    tab_id = params.get('tab_id')
    if tab_id is None:
        tab_id = next(p['tab_id'] for p in session['panes']
                      if p['pane_id'] == params['pane_id'])
    pane = next(p for p in session['panes'] if p['tab_id'] == tab_id)
    snapshot = layout(session, pane['pane_id'])
    return {'layout': {key: snapshot[key] for key in
                       ('workspace_id', 'tab_id', 'focused_pane_id', 'zoomed')} |
            {'root': copy.deepcopy(root_for(session, tab_id))}}


def move(session, params):
    assert params['focus'] is False, params
    pane = next(p for p in session['panes'] if p['pane_id'] == params['pane_id'])
    previous_tab = pane['tab_id']
    previous_root = root_for(session, previous_tab)
    destination = params['destination']
    if session['tabs'][previous_tab].get('zoomed', False):
        return {'move_result': {'changed': False, 'reason': 'zoomed_tab', 'pane': live(pane)}}
    if destination['type'] == 'new_tab':
        session['next_id'] += 1
        tab_id = pane['workspace_id'] + ':tab-staging-' + str(session['next_id'])
        session['tabs'][tab_id] = {'width': session['tabs'][previous_tab]['width'],
                                   'root': None}
        target = None
    else:
        assert destination['type'] == 'tab', destination
        tab_id = destination['tab_id']
        target = destination['target_pane_id']
        root_for(session, tab_id)
    if session['tabs'][tab_id].get('zoomed', False):
        return {'move_result': {'changed': False, 'reason': 'zoomed_tab', 'pane': live(pane)}}
    session['tabs'][previous_tab]['root'] = remove(previous_root, pane['pane_id'])
    pane['tab_id'] = tab_id
    if target is None:
        pane.pop('source_pane_id', None)
        session['tabs'][tab_id]['root'] = leaf(pane['pane_id'])
    else:
        assert target in ids(session['tabs'][tab_id]['root']), params
        pane.update(source_pane_id=target, direction=destination['split'],
                    ratio=destination['ratio'])
        session['tabs'][tab_id]['root'] = insert(session['tabs'][tab_id]['root'],
                                                target, pane['pane_id'],
                                                destination['split'], destination['ratio'])
    if not any(p['tab_id'] == previous_tab for p in session['panes']):
        del session['tabs'][previous_tab]
    return {'move_result': {'changed': True, 'reason': None, 'pane': live(pane)}}


def rpc(session, method, params):
    if method == 'layout.export':
        return export(session, params)
    if method == 'pane.layout':
        return {'layout': layout(session, params['pane_id'])}
    if method == 'pane.move':
        fault = session.get('move_fault', {})
        if fault.get('skip', 0) > 0:
            fault['skip'] -= 1
        elif fault.get('remaining', 0) != 0:
            if fault['remaining'] > 0:
                fault['remaining'] -= 1
            if fault.get('mode') == 'unchanged':
                pane = next(p for p in session['panes'] if p['pane_id'] == params['pane_id'])
                return {'move_result': {'changed': False, 'reason': 'injected_refusal',
                                        'pane': live(pane)}}
            raise RuntimeError('injected pane.move error')
        return move(session, params)
    if method == 'layout.set_split_ratio':
        node = root_for(session, params['tab_id'])
        for index in params['path']:
            node = node['first' if index == 0 else 'second']
        assert node['type'] == 'split', params
        node['ratio'] = params['ratio']
        second = next(p for p in session['panes'] if p['pane_id'] == ids(node['second'])[0])
        second['ratio'] = params['ratio']
        return {'changed': True}
    if method == 'workspace.get':
        return {'workspace': session['workspaces'][params['workspace_id']]}
    if method == 'pane.focus':
        pane = next(p for p in session['panes'] if p['pane_id'] == params['pane_id'])
        for member in session['panes']:
            if member['tab_id'] == pane['tab_id']:
                member['focused'] = member['pane_id'] == pane['pane_id']
        return {'pane': live(pane)}
    raise AssertionError('Unexpected Herdr RPC: ' + method)
"#;

// Model Herdr independently of the launcher's persistence format. Each socket
// has its own live workspace/pane state, while all calls share simulator JSON.
const FAKE_HERDR: &str = r#"#!/usr/bin/env python3
import copy
import fcntl
import json
import os
import sys
import time
from fake_layout import layout, root_for

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
        result = {'layout': layout(session, flag('--pane'))}
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
            # Reconcile both trees before changing membership, then let the
            # shared model prune/insert the moved leaf on its next read.
            previous_tab = pane['tab_id']
            root_for(session, previous_tab)
            root_for(session, source['tab_id'])
            pane.update(tab_id=source['tab_id'], source_pane_id=source['pane_id'], ratio=ratio)
            root_for(session, previous_tab)
            root_for(session, source['tab_id'])
            result = {'move_result': {'changed': True, 'reason': None, 'pane': live(pane)}}
    else:
        raise AssertionError('Unexpected Herdr command: ' + repr(args))
    with open(os.environ['HGG_SIMULATOR'], 'w') as file:
        json.dump(state, file)
    print(json.dumps({'result': result}))
    sys.exit(code)
"#;

// Match Herdr's one-request-per-connection JSON-lines protocol. Every request
// takes the fake CLI's flock; injected failures must also persist their counters
// and call log so a one-time error is actually consumed exactly once.
const FAKE_RPC: &str = r#"
import copy
import fcntl
import json
import os
import socket
import sys

path, simulator = sys.argv[1:]
sys.path.insert(0, os.path.dirname(simulator))
from fake_layout import rpc

with socket.socket(socket.AF_UNIX) as listener:
    listener.bind(path)
    listener.listen()
    print('READY', flush=True)
    while True:
        connection, _ = listener.accept()
        with connection, connection.makefile('rb') as stream:
            line = stream.readline()
            if not line:
                continue
            request = json.loads(line)
            with open(simulator + '.lock', 'a') as lock:
                fcntl.flock(lock, fcntl.LOCK_EX)
                with open(simulator) as file:
                    state = json.load(file)
                session = state['sessions'].setdefault(path, copy.deepcopy(state['seed']))
                call = {'socket': path, 'method': request['method'], 'params': request['params']}
                state.setdefault('rpc_calls', []).append(call)
                try:
                    result = rpc(session, request['method'], request['params'])
                    response = {'id': request['id'], 'result': result}
                except Exception as error:
                    call['error'] = repr(error)
                    response = {'id': request['id'], 'error': {
                        'code': 'fake_rpc_error', 'message': repr(error)}}
                with open(simulator, 'w') as file:
                    json.dump(state, file)
            connection.sendall((json.dumps(response) + '\n').encode())
"#;

struct Fixture {
    temp: tempfile::TempDir,
    repo: PathBuf,
    herdr: PathBuf,
    simulator: PathBuf,
    socket: PathBuf,
    rpc_server: Option<Child>,
}

impl Fixture {
    fn new() -> Self {
        // Unix socket paths have a small platform-dependent length limit.
        let temp = tempfile::Builder::new()
            .prefix("hgg-herdr-")
            .tempdir_in("/tmp")
            .unwrap();
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
        fs::write(temp.path().join("fake_layout.py"), FAKE_LAYOUT).unwrap();
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
            rpc_server: None,
        }
    }

    fn with_rpc() -> Self {
        let mut fixture = Self::new();
        let mut server = Command::new("python3")
            .args(["-u", "-c", FAKE_RPC])
            .arg(&fixture.socket)
            .arg(&fixture.simulator)
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let mut ready = String::new();
        BufReader::new(server.stdout.take().unwrap())
            .read_line(&mut ready)
            .unwrap();
        fixture.rpc_server = Some(server);
        assert_eq!(ready.trim(), "READY", "fake socket server did not start");
        fixture
    }

    fn with_work_panes() -> Self {
        let fixture = Self::with_rpc();
        fixture.update_session(|session| {
            session["panes"].as_array_mut().unwrap().push(json!({
                "pane_id": "w-test:p-second",
                "terminal_id": "w-test:t-second",
                "workspace_id": "w-test",
                "tab_id": "w-test:tab-a",
                "focused": false,
                "source_pane_id": "w-test:p-source",
                "ratio": 0.6
            }));
        });
        fixture
    }

    fn simulator_lock(&self) -> fs::File {
        let lock = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(self.simulator.with_extension("json.lock"))
            .unwrap();
        // Match Python fcntl.flock, including across CLI/server processes.
        assert_eq!(unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) }, 0);
        lock
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
        let _lock = self.simulator_lock();
        serde_json::from_slice(&fs::read(&self.simulator).unwrap()).unwrap()
    }

    fn update_session(&self, update: impl FnOnce(&mut Value)) {
        let _lock = self.simulator_lock();
        let mut state: Value = serde_json::from_slice(&fs::read(&self.simulator).unwrap()).unwrap();
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

    fn rpc_calls(&self, method: &str) -> Vec<Value> {
        self.state()["rpc_calls"]
            .as_array()
            .map(|calls| {
                calls
                    .iter()
                    .filter(|call| call["method"] == method)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
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

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(server) = &mut self.rpc_server {
            let _ = server.kill();
            let _ = server.wait();
        }
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
    assert_eq!(layouts.len(), 3);
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
fn sidebar_toggle_remembers_the_latest_width_for_toggle_and_open() {
    for reopen_mode in ["--toggle-sidebar", "--open-sidebar"] {
        let fixture = Fixture::new();
        let mut current = fixture.run("--toggle-sidebar");
        for ratio in [0.7, 0.6] {
            let id = pane(&current)["pane_id"].as_str().unwrap();
            // Resize immediately before closing, without a viewer state update.
            fixture.update_session(|session| {
                session["panes"]
                    .as_array_mut()
                    .unwrap()
                    .iter_mut()
                    .find(|p| p["pane_id"] == id)
                    .unwrap()["ratio"] = json!(ratio);
            });
            fixture.run("--toggle-sidebar");
            let open_count = fixture.calls(&["plugin", "pane", "open"]).len();
            fixture.run("--sync-sidebar");
            assert_eq!(fixture.calls(&["plugin", "pane", "open"]).len(), open_count);
            current = fixture.run(reopen_mode);
            let reopened_id = pane(&current)["pane_id"].as_str().unwrap();
            let actual = fixture.sidebar(reopened_id)["ratio"].as_f64().unwrap();
            assert!(
                (actual - ratio).abs() < 0.0001,
                "{reopen_mode}: expected {ratio}, got {actual}"
            );
        }
        let other = successful(
            fixture
                .command("--toggle-sidebar")
                .env("HERDR_WORKSPACE_ID", "w-other")
                .env("HERDR_PANE_ID", "w-other:p-source")
                .output()
                .unwrap(),
        );
        assert!(
            (fixture.sidebar(pane(&other)["pane_id"].as_str().unwrap())["ratio"]
                .as_f64()
                .unwrap()
                - 0.8)
                .abs()
                < 0.0001
        );
    }
}

#[test]
fn sidebar_toggle_preserves_preferred_width_when_reopened_in_a_narrow_tab() {
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
            .find(|p| p["pane_id"] == id)
            .unwrap()["ratio"] = json!(0.7);
    });
    fixture.run("--sync-sidebar");
    assert!((fixture.sidebar(id)["ratio"].as_f64().unwrap() - 0.1).abs() < 0.0001);
    fixture.run("--toggle-sidebar");
    let narrow = successful(
        fixture
            .command("--toggle-sidebar")
            .env("HERDR_PANE_ID", "w-test:p-other")
            .output()
            .unwrap(),
    );
    assert!(
        (fixture.sidebar(pane(&narrow)["pane_id"].as_str().unwrap())["ratio"]
            .as_f64()
            .unwrap()
            - 0.1)
            .abs()
            < 0.0001
    );
    fixture.run("--toggle-sidebar");
    fixture.update_session(|session| {
        session["workspaces"]["w-test"]["active_tab_id"] = json!("w-test:tab-a");
    });
    let wide = fixture.run("--toggle-sidebar");
    let actual = fixture.sidebar(pane(&wide)["pane_id"].as_str().unwrap())["ratio"]
        .as_f64()
        .unwrap();
    assert!(
        (actual - 0.7).abs() < 0.0001,
        "expected 60 columns after a constrained toggle, got ratio {actual}"
    );
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
    let fixture = Fixture::with_rpc();
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
fn orphan_sidebar_follows_focus_without_remembering_fullscreen_width() {
    for event in ["tab.focused", "pane.closed", "pane.exited"] {
        let fixture = Fixture::new();
        let first = fixture.run("--open-sidebar");
        let id = pane(&first)["pane_id"].as_str().unwrap();
        fixture.update_session(|session| {
            session["panes"]
                .as_array_mut()
                .unwrap()
                .retain(|pane| pane["pane_id"] != "w-test:p-source");
            session["workspaces"]["w-test"]["active_tab_id"] = json!("w-test:tab-b");
        });
        successful(
            fixture
                .command("--sync-sidebar")
                .env("HERDR_PLUGIN_EVENT", event)
                .env(
                    "HERDR_PLUGIN_EVENT_JSON",
                    r#"{"data":{"workspace_id":"w-test","pane_id":"w-test:p-source"}}"#,
                )
                .output()
                .unwrap(),
        );
        assert_eq!(fixture.sidebar(id)["tab_id"], "w-test:tab-b");
        assert!(
            (fixture.sidebar(id)["ratio"].as_f64().unwrap() - 0.9).abs() < 0.0001,
            "{event}: keep 40 columns, not the orphan tab's full 200-column width"
        );
    }
}

#[test]
fn toggling_an_orphan_sidebar_keeps_its_last_split_width() {
    let fixture = Fixture::new();
    fixture.run("--open-sidebar");
    fixture.update_session(|session| {
        session["panes"]
            .as_array_mut()
            .unwrap()
            .retain(|pane| pane["pane_id"] != "w-test:p-source");
    });
    fixture.run("--toggle-sidebar");
    fixture.update_session(|session| {
        session["workspaces"]["w-test"]["active_tab_id"] = json!("w-test:tab-b");
    });
    let reopened = successful(
        fixture
            .command("--open-sidebar")
            .env("HERDR_PANE_ID", "w-test:p-other")
            .output()
            .unwrap(),
    );
    let id = pane(&reopened)["pane_id"].as_str().unwrap();
    assert!((fixture.sidebar(id)["ratio"].as_f64().unwrap() - 0.9).abs() < 0.0001);
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
        let fixture = if terminal_reused {
            Fixture::with_rpc()
        } else {
            Fixture::new()
        };
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
        let fixture = if terminal_reused {
            Fixture::with_rpc()
        } else {
            Fixture::new()
        };
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
        let work_tree = fixture.state()["sessions"][fixture.socket.to_str().unwrap()]["tabs"]
            ["w-test:tab-a"]["root"]
            .clone();
        let replacement = fixture.run("--open-sidebar");
        assert_ne!(pane(&replacement)["pane_id"], pane(&first)["pane_id"]);
        assert_eq!(pane(&fixture.run("--open-sidebar")), pane(&replacement));
        assert_eq!(fixture.calls(&["plugin", "pane", "open"]).len(), 2);
        // Same-tab reparenting uses socket pane.move, not CLI relocation.
        assert!(fixture.calls(&["pane", "move"]).is_empty());
        if terminal_reused {
            assert_eq!(fixture.sidebar(id)["terminal_id"], "unrelated-terminal");
            let moves = fixture.rpc_calls("pane.move");
            assert_eq!(moves.len(), 2, "stage and restore the unrelated work pane");
            assert!(moves.iter().all(|call| call["params"]["pane_id"] == id));
            assert_eq!(moves[0]["params"]["destination"]["type"], "new_tab");
            assert_eq!(moves[1]["params"]["destination"]["tab_id"], "w-test:tab-a");
            assert_eq!(fixture.rpc_calls("layout.set_split_ratio").len(), 1);
            assert_eq!(fixture.rpc_calls("pane.focus").len(), 1);
            let state = fixture.state();
            let tabs = &state["sessions"][fixture.socket.to_str().unwrap()]["tabs"];
            let wrapped = &tabs["w-test:tab-a"]["root"];
            assert_eq!(wrapped["direction"], "right");
            assert_eq!(wrapped["first"], work_tree);
            assert_eq!(wrapped["second"]["pane_id"], pane(&replacement)["pane_id"]);
            assert!(
                tabs.as_object()
                    .unwrap()
                    .keys()
                    .all(|tab| !tab.contains(":tab-staging-")),
                "staging tab must disappear when its final pane returns"
            );
        } else {
            assert!(fixture.rpc_calls("layout.export").is_empty());
        }
    }
}

#[test]
fn multi_pane_open_recovers_once_after_a_partial_rpc_move_failure() {
    let fixture = Fixture::with_work_panes();
    fixture.update_session(|session| {
        // Stage the second work pane, then fail its first attempt to return.
        session["move_fault"] = json!({"skip": 1, "remaining": 1, "mode": "error"});
    });
    let opened = fixture.run("--open-sidebar");
    let graph = pane(&opened);
    let moves = fixture.rpc_calls("pane.move");
    assert_eq!(
        moves.len(),
        4,
        "initial stage/error, then one stage/restore recovery"
    );
    assert_eq!(
        moves
            .iter()
            .filter(|call| call["error"].is_string())
            .count(),
        1
    );
    assert_eq!(fixture.calls(&["plugin", "pane", "open"]).len(), 1);
    assert!(fixture.calls(&["pane", "close"]).is_empty());
    for (id, terminal) in [
        ("w-test:p-source", "w-test:t-source"),
        ("w-test:p-second", "w-test:t-second"),
    ] {
        let work = fixture.sidebar(id);
        assert_eq!(work["terminal_id"], terminal);
        assert_eq!(work["tab_id"], "w-test:tab-a");
    }
    let state = fixture.state();
    let session = &state["sessions"][fixture.socket.to_str().unwrap()];
    let root = &session["tabs"]["w-test:tab-a"]["root"];
    assert_eq!(root["direction"], "right");
    assert_eq!(root["second"]["pane_id"], graph["pane_id"]);
    assert_eq!(
        root["first"],
        json!({"type": "split", "direction": "right", "ratio": 0.6,
               "first": {"type": "pane", "pane_id": "w-test:p-source"},
               "second": {"type": "pane", "pane_id": "w-test:p-second"}})
    );
    assert_eq!(session["move_fault"]["remaining"], 0);
    assert!(
        session["tabs"]
            .as_object()
            .unwrap()
            .keys()
            .all(|tab| !tab.contains(":tab-staging-"))
    );
    assert_eq!(fixture.sidebar("w-test:p-source")["focused"], true);
    assert_eq!(
        pane(&fixture.run("--open-sidebar"))["pane_id"],
        graph["pane_id"]
    );
    assert_eq!(fixture.calls(&["plugin", "pane", "open"]).len(), 1);
    assert_eq!(fixture.rpc_calls("pane.move").len(), 4);
}

#[test]
fn permanent_rpc_move_refusal_keeps_user_panes_and_the_registered_viewer() {
    let fixture = Fixture::with_work_panes();
    fixture.update_session(|session| {
        session["move_fault"] = json!({"skip": 1, "remaining": -1, "mode": "unchanged"});
    });
    let failed = fixture.command("--open-sidebar").output().unwrap();
    assert!(!failed.status.success());
    assert!(failed.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&failed.stderr);
    assert!(stderr.contains("복구 실패"), "{stderr}");
    assert!(stderr.contains("Git Graph layout"), "{stderr}");
    assert!(stderr.contains("닫지 않았습니다"), "{stderr}");
    assert_eq!(
        fixture.rpc_calls("pane.move").len(),
        3,
        "initial stage/refusal, then exactly one refused recovery"
    );
    for (id, terminal) in [
        ("w-test:p-source", "w-test:t-source"),
        ("w-test:p-second", "w-test:t-second"),
    ] {
        assert_eq!(fixture.sidebar(id)["terminal_id"], terminal);
    }
    assert!(
        fixture.sidebar("w-test:p-second")["tab_id"]
            .as_str()
            .unwrap()
            .contains(":tab-staging-")
    );
    assert!(fixture.calls(&["pane", "close"]).is_empty());
    assert!(fixture.rpc_calls("pane.close").is_empty());
    let state = fixture.state();
    let graphs: Vec<_> = state["sessions"][fixture.socket.to_str().unwrap()]["panes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|pane| pane["entrypoint"] == "sidebar")
        .collect();
    assert_eq!(graphs.len(), 1);
    // The graph was registered before placement failed. Another action must
    // reuse it, not spawn another viewer or close the stranded work pane.
    let reused = fixture.run("--open-sidebar");
    assert_eq!(pane(&reused)["pane_id"], graphs[0]["pane_id"]);
    assert_eq!(pane(&reused)["terminal_id"], graphs[0]["terminal_id"]);
    assert_eq!(fixture.calls(&["plugin", "pane", "open"]).len(), 1);
    assert_eq!(fixture.rpc_calls("pane.move").len(), 3);
    assert_eq!(
        fixture.sidebar("w-test:p-second")["terminal_id"],
        "w-test:t-second"
    );
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
