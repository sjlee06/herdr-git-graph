# Development

[Back to README](../README.md)

The viewer is built with Rust and Ratatui.

## Build and link locally

Source builds require Rust stable with Cargo on `PATH`, Git, and a macOS or Linux environment. Install Rust with [rustup](https://rustup.rs/) if needed. End-user installation uses `scripts/install.sh` and requires no Rust toolchain.

```bash
git clone https://github.com/sjlee06/herdr-git-graph.git
cd herdr-git-graph
sh scripts/build.sh
herdr plugin link .
```

The build script runs `cargo build --release --locked` and copies the executable to `bin/herdr-git-graph`, the path declared by the plugin manifest. `CARGO_TARGET_DIR` is supported. Generated binaries and build output are ignored by Git.

After changing Rust code, rebuild and reopen the viewer. If you change the manifest, relink the plugin so Herdr reads it again.

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

코드를 수정할 때마다 `sh scripts/build.sh --debug`로 다시 빌드하고, 그래프에서 `q`로 닫은 뒤 다시 여세요. `r`은 Git 데이터와 터미널 색상을 다시 조회하며 실행 파일을 교체하지는 않습니다. manifest를 수정했을 때는 `herdr plugin link .`도 다시 실행합니다.

배포된 릴리스로 돌아가려면 열린 그래프를 `q`로 닫고, 로컬 링크를 먼저 해제한 뒤 설치하세요. 링크된 상태에서 GitHub 설치를 실행하면 `already linked from a local path` 오류가 발생합니다.

```bash
herdr plugin unlink herdr.git-graph
herdr plugin install sjlee06/herdr-git-graph
```

터미널을 밝은 테마와 어두운 테마로 각각 바꾸고 그래프에서 `r`을 누르세요. 일반 행 배경이 옆 패널과 이어지는지, 선택 행이 보이는지, 곡선 배경에 검은 사각형이 없는지 확인합니다. `/` 검색·한글 입력·방향키·마우스·창 크기 변경·`q` 종료도 확인하세요. `--theme terminal`은 색상 조회 없는 대체 경로를 확인하는 옵션이며 문자 그래프를 사용합니다.

개인 Herdr 세션에 플러그인을 연결하지 않고 실제 실행 경로만 검증하려면 빌드 후 `python3 tests/herdr_live.py`를 실행하세요. 테스트가 별도 세션과 임시 설정을 만들고 정리합니다. 실제 바깥 터미널에서 보이는 색상·투명도·곡선의 육안 확인은 위 수동 확인이 필요합니다.

## Checks

```bash
python3 tests/install.py
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
sh scripts/build.sh
python3 tests/smoke.py ./bin/herdr-git-graph --work /tmp/hgg-smoke
```

The Python smoke tests use only the standard library. Keep the work path short: Unix domain socket paths have a small length limit, especially on macOS.

When Herdr is installed, also run `python3 tests/herdr_live.py` after building. It starts a separate headless test session with isolated XDG directories, invokes the real sidebar and full-view actions, checks the resulting panes and graph output, and stops only its own test server. Run this when changing plugin launch arguments; a mock CLI does not enforce Herdr's placement rules.

The [CI workflow](../.github/workflows/ci.yml) runs on macOS and Linux. Tests cover graph topology, real temporary Git repositories, branch filters, empty commits, detached HEAD, linked worktrees, shallow clones, Unicode search, and external-diff suppression. Working tree tests include partial staging, untracked/ignored files, renames, deletions, conflicts, unborn HEAD, bounded patches, and index preservation. Refresh tests check unchanged-status edits, selection/scroll preservation, stale responses, single pending polls, and backoff. PTY tests exercise live edits and commits without manual reload, auto-refresh opt-out, navigation, resize, termination cleanup, graphics transport, and text fallback with a mock Herdr server.

See [validation notes](VALIDATION.md) for recorded results and live terminal testing coverage.

## Preview assets

The README leads with `docs/preview-sidebar.png`, a full Herdr terminal showing workspace navigation, source code, a test terminal, and the graph sidebar together. It is rendered from the actual ANSI output and Kitty graphics frames of an isolated Herdr demo session. The editor shows the unmodified `stroke` function in `src/graphics.rs`; the terminal shows a successful `cargo test --lib graph::tests` run. The graph uses built-in sample history with `--demo --sidebar --theme classic --renderer curves`.

The scene uses a 160-column × 42-row terminal, a 24-column Herdr workspace list, and a 40-column graph split including the host borders and scrollbar. The remaining 96 columns are split vertically between the editor and test terminal. `docs/preview-sidebar.svg` is 1600 × 840 pixels; its 2× PNG is 3200 × 1680 pixels and fills the available README width.

Generate the separate full-view and text-mode previews from the same built-in data using the Ratatui test backend and the app's curve renderer:

```bash
./bin/herdr-git-graph --demo --theme classic --renderer curves \
  --width 140 --height 56 --snapshot docs/preview.svg
./bin/herdr-git-graph --demo --theme classic --renderer text \
  --width 140 --height 56 --snapshot docs/preview-text.svg
```

Rasterize `preview.svg` to `preview.png` at 2× scale with an SVG renderer supporting system fonts and embedded PNG images, such as [resvg](https://github.com/linebender/resvg). The full-view PNG is 2520 × 2240 pixels and includes the colored patch in the inspector.

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
| `src/graphics.rs` | tiny-skia curves and Herdr PNG streams |
| `src/theme.rs` | Native/classic styles, OSC color parsing, shared RGB palette |
| `src/theme_probe.rs` | Asynchronous terminal queries and color-reply filtering |
| `src/herdr.rs` | Repository context, plugin action, socket requests |
| `tests/repository.rs` | Integration tests using temporary Git repositories |
| `tests/herdr.rs` | Tab/sidebar launch arguments, repository context, CLI errors |
| `tests/herdr_live.py` | Optional real Herdr action and pane integration in an isolated session |
| `tests/smoke.py` | PTY and mock Herdr graphics tests |

Graph layout is separate from rendering. Keep parent relationships intact across branch filtering and commit limits, and retain terminal cleanup and text fallback when changing rendering code.

## Contributing

Keep changes focused and include relevant checks. For a rendering bug, include the Herdr version, outer terminal, local or remote connection, renderer mode, and a screenshot or minimal reproduction. For a graph bug, a small synthetic repository reproducing the topology is especially useful.

## Releases

Update the package version in `Cargo.toml`, `Cargo.lock`, and `herdr-plugin.toml` together, then push a matching `vX.Y.Z` tag. The [release workflow](../.github/workflows/release.yml) builds and tests native arm64 and x86_64 binaries for macOS and Linux before publishing all four binaries and their SHA-256 files to GitHub Releases. macOS builds target 11.0; Linux builds use Ubuntu 22.04 (glibc 2.35).

`scripts/install.sh` selects the platform and downloads the exact manifest version, verifies the checksum and executable, then replaces `bin/herdr-git-graph`. It preserves an existing binary on failure. To explicitly build during a Herdr install, set `HERDR_GIT_GRAPH_BUILD_FROM_SOURCE=1`; local development can call `sh scripts/build.sh` directly. Download failures do not silently start a source build.

`tests/install.py` checks all platform selections without using Cargo, both checksum tools, download failures, checksum corruption, preservation of an existing installation, temporary-file cleanup, and source-build error messages.
