# Development

[Back to README](../README.md)

## Build and link locally

Use Rust stable, Git, and a macOS or Linux environment.

```bash
git clone https://github.com/sjlee06/herdr-git-graph.git
cd herdr-git-graph
sh scripts/build.sh
herdr plugin link .
```

The build script runs `cargo build --release --locked` and copies the executable to `bin/herdr-git-graph`, the path declared by the plugin manifest. `CARGO_TARGET_DIR` is supported. Generated binaries and build output are ignored by Git.

After changing Rust code, rebuild and reopen the viewer. If you change the manifest, relink the plugin so Herdr reads it again.

```bash
./bin/herdr-git-graph --demo
./bin/herdr-git-graph --repo /path/to/repository --check
```

`--demo` uses built-in sample data. `--check` summarizes the repository without opening a TUI. On macOS, `Demo.command` opens the sample view, building first if the executable is missing.

## Checks

```bash
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
sh scripts/build.sh
python3 tests/smoke.py ./bin/herdr-git-graph --work /tmp/hgg-smoke
```

The Python smoke tests use only the standard library. Keep the work path short: Unix domain socket paths have a small length limit, especially on macOS.

The [CI workflow](../.github/workflows/ci.yml) runs on macOS and Linux. Tests cover graph topology, real temporary Git repositories, branch filters, empty commits, detached HEAD, linked worktrees, shallow clones, Unicode search, and external-diff suppression. PTY tests exercise navigation, resize, termination cleanup, graphics transport, and text fallback with a mock Herdr server.

## Preview assets

Generate the UI from the Ratatui test backend and the same curve renderer used by the app:

```bash
cargo run -- --demo --renderer curves --snapshot docs/preview.svg
cargo run -- --demo --renderer text --snapshot docs/preview-text.svg
cargo run -- --demo --graph-png docs/curves.png
```

The README's `docs/preview.png` is a rasterized copy of `preview.svg` for consistent GitHub display. Regenerate it with an SVG renderer supporting system fonts and embedded PNG images, such as [resvg](https://github.com/linebender/resvg). These are demo snapshots, not captures of a live Herdr window.

## Source layout

| File | Responsibility |
| --- | --- |
| `src/main.rs` | CLI, terminal lifecycle, input loop |
| `src/app.rs` | View state, search, background Git requests |
| `src/git.rs` | Bounded Git subprocesses, log/ref parsing, diffs |
| `src/graph.rs` | Lane assignment from parent commit IDs |
| `src/ui.rs` | Ratatui panels, Unicode graph, SVG snapshots |
| `src/graphics.rs` | tiny-skia curves and Herdr PNG streams |
| `src/herdr.rs` | Repository context, plugin action, socket requests |
| `tests/repository.rs` | Integration tests using temporary Git repositories |
| `tests/smoke.py` | PTY and mock Herdr graphics tests |

Graph layout is separate from rendering. Keep parent relationships intact across branch filtering and commit limits, and retain terminal cleanup and text fallback when changing rendering code.

## Contributing

Keep changes focused and include relevant checks. For a rendering bug, include the Herdr version, outer terminal, local or remote connection, renderer mode, and a screenshot or minimal reproduction. For a graph bug, a small synthetic repository reproducing the topology is especially useful.
