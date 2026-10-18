# Development

[Back to README](../README.md)

The viewer is built with Rust and Ratatui.

## Build and link locally

Source builds require Rust stable with Cargo on `PATH`, Git, and a macOS or Linux environment. Install Rust with [rustup](https://rustup.rs/) if needed. End-user installation uses `scripts/install.sh` and requires no Rust toolchain. Herdr integration requires 0.9.5+, the tested baseline for the shared sidebar's tab-focus hook and pane-movement APIs.

```bash
git clone https://github.com/sjlee06/herdr-git-graph.git
cd herdr-git-graph
sh scripts/build.sh
herdr plugin link .
```

The build script runs `cargo build --release --locked` and copies the executable to `bin/herdr-git-graph`, the path declared by the plugin manifest. `CARGO_TARGET_DIR` is supported. Generated binaries and build output are ignored by Git.

After changing Rust code, rebuild and reopen the viewer. Close the shared sidebar before invoking its action again: repeating the action reuses the running process, so it does not load the rebuilt binary. Older running viewers must also be reopened to begin saving view and width state for tab-close recovery. If you change the manifest, run `herdr plugin link .` again so Herdr refreshes its action and hook registrations.

For faster local builds with debug symbols, use `sh scripts/build.sh --debug`. This builds `target/debug/herdr-git-graph` and atomically replaces `bin/herdr-git-graph`, so an already-open viewer can keep running until you close it. Running panes continue using the old executable until reopened. The default build remains `--release`.

```bash
./bin/herdr-git-graph --demo
./bin/herdr-git-graph --demo --sidebar
./bin/herdr-git-graph --repo /path/to/repository --check
```

`--demo` uses built-in sample data. `--check` summarizes the repository without opening a TUI.

## main 푸시 전 로컬 디버깅

Rust/Cargo가 `PATH`에 있어야 소스를 다시 빌드할 수 있습니다. 아직 없다면 [rustup](https://rustup.rs/)으로 설치하고 새 셸을 여세요. 빌드된 `bin/herdr-git-graph` 실행에는 Rust가 필요하지 않습니다.

```bash
# 저장소 루트에서 빠르게 빌드
sh scripts/build.sh --debug

# 일반 터미널: 현재 테마, 실제 저장소, 사이드바 확인
RUST_BACKTRACE=1 ./bin/herdr-git-graph --repo .
RUST_BACKTRACE=1 ./bin/herdr-git-graph --repo . --sidebar

# 데모로 기존 테마와 비교
./bin/herdr-git-graph --demo --theme classic
./bin/herdr-git-graph --demo --theme auto
```

실제 Herdr 탭·사이드바와 곡선은 **Herdr 안의 터미널**에서 확인합니다. 아래 `link`는 동일 ID 플러그인의 실행 경로를 이 로컬 폴더로 바꿉니다. `install`은 릴리스 실행 파일을 다운로드하므로 로컬 코드 확인에는 `link`를 사용하세요. `link` 자체는 빌드를 실행하지 않습니다.

```bash
herdr plugin link .
herdr plugin action invoke herdr.git-graph.open
herdr plugin action invoke herdr.git-graph.sidebar

# 열리지 않으면 액션 실행 로그 확인
herdr plugin log list --plugin herdr.git-graph
```

코드를 수정할 때마다 `sh scripts/build.sh --debug`로 다시 빌드하고, 그래프에서 `q`로 닫은 뒤 다시 여세요. 공유 사이드바는 액션을 반복해도 실행 중인 프로세스를 재사용하므로 반드시 닫았다가 열어야 새 실행 파일이 적용됩니다. `r`은 Git 데이터와 터미널 색상을 다시 조회하며 실행 파일을 교체하지는 않습니다. manifest를 수정했을 때는 `herdr plugin link .`도 다시 실행해 액션과 hook 등록을 갱신합니다.

배포된 릴리스로 돌아가려면 열린 그래프를 `q`로 닫고, 로컬 링크를 먼저 해제한 뒤 설치하세요. 링크된 상태에서 GitHub 설치를 실행하면 `already linked from a local path` 오류가 발생합니다.

```bash
herdr plugin unlink herdr.git-graph
herdr plugin install sjlee06/herdr-git-graph
```

터미널을 밝은 테마와 어두운 테마로 각각 바꾸고 그래프에서 `r`을 누르세요. 일반 행 배경이 옆 패널과 이어지는지, 선택 행이 보이는지, 곡선 배경에 검은 사각형이 없는지 확인합니다. `/` 검색·한글 입력·방향키·마우스·창 크기 변경·`q` 종료도 확인하세요. `--theme terminal`은 색상 조회 없는 대체 경로를 확인하는 옵션이며 문자 그래프를 사용합니다.

공유 사이드바는 액션을 반복해도 패널이 늘어나지 않는지, 다른 탭에서 실행하면 그 탭으로 이동하는지 확인하세요. 탭 전환 시 포커스, 선택한 커밋, 스크롤, 검색, 테마와 수동 조절한 너비가 유지되는지도 확인합니다. 좁은 대상 패널에서는 Herdr의 10–90% 분할 비율 제한으로 실제 너비가 줄어들 수 있지만, 넓은 탭으로 돌아오면 저장한 목표 너비가 복원되어야 합니다. 다른 작업 공간에는 별도의 사이드바가 열려야 합니다. `q`나 패널 닫기로 사이드바를 직접 끈 뒤에는 탭을 전환하거나 해당 탭을 닫아도 다시 열리지 않아야 합니다. 반대로 사이드바가 있는 탭을 닫으면 같은 작업 공간의 남은 탭에 하나만 다시 열려야 합니다. 이때 새 프로세스에서 원래 저장소와 테마, 선택·스크롤·검색 상태, 목표 너비가 복원되는지 확인하세요. 좁은 탭을 닫은 뒤에도 줄어든 실제 너비가 아닌 목표 너비로 복구되어야 합니다. Git 저장소가 아닌 탭에서도 복구되는지, 마지막 탭을 닫으면 다른 작업 공간에 사이드바가 생기지 않는지도 확인하세요. 확대된 탭 때문에 이동이 실패하면 기존 프로세스와 확대 상태가 유지되어야 하며, 확대를 해제한 뒤 탭 전환이나 액션으로 다시 이동할 수 있어야 합니다.

개인 Herdr 세션에 플러그인을 연결하지 않고 실제 실행 경로만 검증하려면 빌드 후 `python3 tests/herdr_live.py`를 실행하세요. 테스트가 별도 세션과 임시 설정을 만들고 정리합니다. 실제 바깥 터미널에서 보이는 색상·투명도·곡선의 육안 확인은 위 수동 확인이 필요합니다.

## Checks

The launcher regression tests use `python3` to simulate Herdr; Python is not required to run the plugin.

```bash
python3 tests/install.py
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
sh scripts/build.sh
```

When Herdr 0.9.5+ is installed, also run `python3 tests/herdr_live.py` after building. It starts a separate headless test session with isolated XDG directories and stops only its own test server. It checks sidebar toggle open/close/reopen, last-width persistence including constrained-to-wide reopen, and hook opt-out, sidebar reuse, cross-tab following and rapid switching, stable pane/terminal identity during moves, selection/scroll and width preservation, focus, workspace isolation, zoomed-source move failure and recovery, containing-tab close recovery of the view, active search, and target width to a non-Git tab, narrow-to-wide width restoration, recovery after closing a constrained tab, explicit close/opt-out followed by tab deletion, last-tab close, graph output, and the full-view action. Run it when changing plugin launch or synchronization behavior; a mock CLI does not enforce Herdr's placement rules.

The [CI workflow](../.github/workflows/ci.yml) runs on macOS and Linux. Tests cover graph topology, real temporary Git repositories, branch filters, empty commits, detached HEAD, linked worktrees, shallow clones, Unicode search, and external-diff suppression. Working tree tests include partial staging, untracked/ignored files, renames, deletions, conflicts, unborn HEAD, bounded patches, and index preservation. Refresh tests check unchanged-status edits, selection/scroll preservation, stale responses, single pending polls, and backoff. View-state tests cover selection/viewport/search/pan roundtrips, OID anchors across inserted or removed commits, empty history, malformed values, and draw-time visibility adjustments.

See [validation notes](VALIDATION.md) for recorded results and live terminal testing coverage.

## Preview assets

Both READMEs compare dark (Classic) and light (Tomorrow) demos side by side. The sidebar pair is `docs/preview-sidebar-dark.png` and `docs/preview-sidebar-light.png`, both 720 × 1040 pixels (40×26 cells). The full-view pair is `docs/preview.png` and `docs/preview-light.png`, both 2520 × 2240 pixels (140×56 cells). Each image links to its full-size PNG.

The linked `docs/preview-sidebar.png` is a demo composite showing workspace navigation, source code, a test terminal, and the graph sidebar together. The workspace frame, editor, and test output are preserved from the original isolated Herdr session capture; they are not a new source or test capture. `scripts/preview.py` replaces only the graph pane's content with the current viewer's headless snapshot, including branch/tag badges, one-line subjects, and no history hashes. The graph uses built-in sample history with `--demo --sidebar --theme classic --renderer curves`.

The scene uses a 160-column × 42-row terminal, a 24-column Herdr workspace list, and a 40-column graph split including the host borders and scrollbar. The remaining 96 columns are split vertically between the editor and test terminal. `docs/preview-sidebar.svg` is 1600 × 840 pixels; its 2× PNG is 3200 × 1680 pixels.

Rebuild the viewer, then refresh the four dark SVGs from the same built-in data using the Ratatui test backend and the app's curve renderer:

```bash
sh scripts/build.sh
python3 scripts/preview.py
```

The script preserves the captured workspace frame in `docs/preview-sidebar.svg` and replaces its 38-column × 39-row graph content. It maps the snapshot's 9×20-pixel cells to the frame's 10×20-pixel cells. Keep that SVG as the input frame; rerunning the script replaces the graph rather than stacking another copy.

To generate just the separate full-view and text-mode previews:

```bash
./bin/herdr-git-graph --demo --theme classic --renderer curves \
  --width 140 --height 56 --snapshot docs/preview.svg
./bin/herdr-git-graph --demo --theme classic --renderer text \
  --width 140 --height 56 --snapshot docs/preview-text.svg
```

Rasterize the dark previews at 2× scale with an SVG renderer supporting system fonts, nested SVGs, and embedded PNG images, such as [resvg](https://github.com/linebender/resvg):

```bash
resvg --zoom 2 docs/preview.svg docs/preview.png
resvg --zoom 2 docs/preview-sidebar.svg docs/preview-sidebar.png
resvg --zoom 2 docs/preview-sidebar-dark.svg docs/preview-sidebar-dark.png
```

The full-view PNG is 2520 × 2240 pixels and includes the commit hash and colored patch in the inspector; hashes do not appear in History. The sidebar PNG is 3200 × 1680 pixels. Check both images for readable current/local/remote/tag badges, subject truncation, Korean text, and curve alignment after regenerating them.

The light demos use the same renderer with Ghostty's bundled Tomorrow foreground (`#4d4d4c`), background (`#ffffff`), and ANSI palette, as if returned by terminal color queries. These are terminal colors, not Herdr's workspace UI theme. They are headless examples, not live terminal captures; no `--theme light` option or terminal configuration change is needed. The sidebar is a standalone 40×26-cell view rather than the full workspace composite.

```bash
cargo run --locked --example preview-light
resvg --zoom 2 docs/preview-light.svg docs/preview-light.png
resvg --zoom 2 docs/preview-sidebar-light.svg docs/preview-sidebar-light.png
```

For a standalone sidebar layout check, export to a temporary path so it does not replace the full-terminal README image:

```bash
./bin/herdr-git-graph --demo --sidebar --theme classic --renderer curves \
  --width 40 --height 26 --snapshot /tmp/herdr-git-graph-sidebar.svg
```

This standalone export has a 40-column content area; the full-terminal image uses a real 40-column Herdr split with less content space after host borders and the scrollbar. Actual theme colors follow the host terminal by default.

## Source layout

| File | Responsibility |
| --- | --- |
| `src/main.rs` | CLI, terminal lifecycle, input loop |
| `src/app.rs` | View state, search, background Git requests |
| `src/git.rs` | Bounded Git subprocesses, log/ref parsing, diffs |
| `src/graph.rs` | Lane assignment from parent commit IDs |
| `src/ui.rs` | Ratatui panels, Unicode graph, SVG snapshots |
| `src/ui/refs.rs` | Branch/tag badge classification, priority, styling, and overflow |
| `src/graphics.rs` | tiny-skia curves and direct Kitty graphics PNG output |
| `src/theme.rs` | Native/classic styles, OSC color parsing, shared RGB palette |
| `src/theme_probe.rs` | Asynchronous terminal queries and color-reply filtering |
| `src/herdr.rs` | Repository context, plugin pane launch, and sidebar sizing |
| `src/herdr/sidebar.rs` | Workspace-shared sidebar registry, file locking, viewer snapshots, width tracking, reuse, and tab synchronization/recovery |
| `src/herdr/view_state.rs` | History-view JSON snapshots and safe OID-anchored restoration |
| `tests/repository.rs` | Integration tests using temporary Git repositories |
| `tests/herdr.rs` | Launcher arguments, shared-sidebar reuse/movement, workspace/session isolation, concurrent opens, stale records, and CLI errors |
| `tests/herdr_live.py` | Optional real Herdr action and pane integration in an isolated session |

The shared-sidebar registry uses hashed `.json` and `.lock` filenames under `$HERDR_PLUGIN_STATE_DIR/shared-sidebars/`, keyed by Herdr server socket and workspace. When that variable is unset, it falls back to `$XDG_STATE_HOME/herdr-git-graph/shared-sidebars/` or `$HOME/.local/state/herdr-git-graph/shared-sidebars/`. A file lock serializes sidebar actions, synchronization hooks, and viewer updates. Records contain pane, terminal, and containing-tab IDs, the original repository and theme, and `width`, `actual_width`, and `view` fields.

`width` is the target outer width, initially 40 columns; `actual_width` tracks the constrained geometry. Initial sizing and moves use Herdr's 10–90% split-ratio limits, without a separate half-target-pane cap. A move or recreation saves its constrained actual width without replacing the target, so a narrow destination does not prevent width restoration on a wider tab. Later user/host geometry changes update the target. `view` stores `selected_oid` and `top_oid` with `selected` and `top` index fallbacks, plus `graph_offset`, `query`, and `searching`. Restoration finds the saved OIDs in the newly loaded history, falls back to clamped indices, and bounds the horizontal offset. `ui::draw` adjusts selection visibility for the new terminal height; sidebar state does not include branch filters or detail panels.

Action-managed viewers receive the internal `HERDR_GIT_GRAPH_SHARED=1` marker and use `SidebarSession` to restore and save the matching record. Direct pane opens and standalone `--sidebar` runs do not participate. The viewer saves after the initial draw and when persisted UI state changes. It checks Herdr geometry on terminal resize and once per second, including logical layout changes without a PTY resize, and does not rewrite unchanged records. Viewers already running an older binary must be closed and reopened to enable this persistence.

Pane and terminal IDs are checked against live panes before reuse. When a pane is missing but its tab remains, synchronization clears the record without reopening. When the containing tab has been deleted and a valid destination remains in the same workspace, it creates one new process on the focused workspace's active tab using the saved launch settings, view, and target width. This is not server-restart/session recovery; pane/terminal identity guards reject stale reused IDs. `tab.closed` hooks also run synchronization because closing a tab need not emit a focus event. `pane.closed` and `pane.exited` hooks clear matching records for explicit opt-out, and the viewer clears its record before an explicit quit; delayed events for old panes do not clear replacements. Removed workspaces and legacy records without recovery settings are cleared, not restored. Missing newer view/width fields use safe defaults. Recreation can briefly change the layout, and cross-tab moves remain subject to Herdr's zoom restrictions; the plugin does not automatically unzoom tabs.

Graph layout is separate from rendering. Keep parent relationships intact across branch filtering and commit limits, and retain terminal cleanup and text fallback when changing rendering code.

## Contributing

Keep changes focused and include relevant checks. For a rendering bug, include the Herdr version, outer terminal, local or remote connection, renderer mode, and a screenshot or minimal reproduction. For a graph bug, a small synthetic repository reproducing the topology is especially useful.

## Releases

Update the package version in `Cargo.toml`, `Cargo.lock`, and `herdr-plugin.toml` together, then push a matching `vX.Y.Z` tag. The [release workflow](../.github/workflows/release.yml) builds and tests native arm64 and x86_64 binaries for macOS and Linux before publishing all four binaries and their SHA-256 files to GitHub Releases. macOS builds target 11.0; Linux builds use Ubuntu 22.04 (glibc 2.35).

`scripts/install.sh` selects the platform and downloads the exact manifest version, verifies the checksum and executable, then replaces `bin/herdr-git-graph`. It preserves an existing binary on failure. To explicitly build during a Herdr install, set `HERDR_GIT_GRAPH_BUILD_FROM_SOURCE=1`; local development can call `sh scripts/build.sh` directly. Download failures do not silently start a source build.

`tests/install.py` checks all platform selections without using Cargo, both checksum tools, download failures, checksum corruption, preservation of an existing installation, temporary-file cleanup, and source-build error messages.
