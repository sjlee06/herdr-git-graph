# Herdr Git Graph

**Keep Git history beside your code. Open the full view to inspect a diff.**

[![CI](https://github.com/sjlee06/herdr-git-graph/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/sjlee06/herdr-git-graph/actions/workflows/ci.yml)
[![MIT License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Herdr Plugin](https://img.shields.io/badge/Herdr-plugin-5eead4)](https://herdr.dev/docs/plugins/)

English · [한국어](README.ko.md)

A read-only Git graph viewer built with Rust and Ratatui. Keep a compact graph on the right while you work, follow branches and merges, and search commits without switching tabs. Open the full view for branch filters and commit diffs. Compatible Herdr panes display smooth curves; other terminals use a colored Unicode graph.

<p align="center">
  <img src="docs/preview-sidebar.png" width="360" alt="40-column Git Graph sidebar showing demo branches, merge curves, commit messages, and keyboard hints">
</p>

*The sidebar in demo mode: a compact history graph beside your work. Generated at 40 columns with the classic theme; Herdr adds its own borders and scrollbar when opening a split.*

## Features

- **Graph sidebar** — opens on the right at a default width of 40 columns and keeps focus on your current pane. Search, navigate, and follow local changes while you work.
- **Branch and merge graph** — colored lanes, branch and tag labels, and support for detached HEAD and linked worktrees.
- **Commit inspector** — commit metadata, changed-file statistics, and colored diffs in one view.
- **Uncommitted changes** — a working tree node above HEAD, with staged/unstaged diffs, untracked file names, and conflict counts.
- **Live updates** — background checks every 2 seconds reflect local file, commit, and branch changes while preserving selection and scroll position.
- **Search and navigation** — find a message, author, ref, or hash; navigate with the keyboard or mouse.
- **Smooth curves with a text fallback** — antialiased Bézier connections through Herdr's graphics API, plus a Unicode renderer for ordinary terminals.
- **Terminal theme integration** — uses the current foreground/background and ANSI palette, with transparent curve backgrounds and light/dark support. `--theme classic` restores the original look. See [theme behavior and fallbacks](docs/USAGE.md#terminal-theme).

## Install

Requires **Herdr 0.9.0+**, **Git 2.31+**, and `curl`. **Rust and Cargo are not required.** The installer downloads a matching [release binary](https://github.com/sjlee06/herdr-git-graph/releases) and verifies its SHA-256 checksum using `shasum` or `sha256sum`.

Prebuilt releases support **macOS 11+** and **Linux with glibc 2.35+** (for example, Ubuntu 22.04+), on Apple Silicon/ARM64 and x86_64. For other systems, see [source builds](docs/DEVELOPMENT.md).

```bash
herdr plugin install sjlee06/herdr-git-graph
```

From a Herdr workspace containing a Git repository, open the sidebar beside your current pane:

```bash
herdr plugin action invoke herdr.git-graph.sidebar
```

The sidebar opens at **40 columns**, with room for `Uncommitted changes` next to the graph and for Herdr’s borders and scrollbar. Available space and Herdr’s split limits still apply. Drag the divider or use `prefix+r` to resize it; use `prefix+l` to focus it, then `q` to close.

`/ search`, `? help`, and `q quit` remain visible at the minimum supported content size of 24 columns × 8 rows. Help wraps and scrolls with `↑↓`, `j/k`, or `PgUp/PgDn`; `Esc`, `?`, or `q` closes it. Long search input keeps the cursor visible.

### Full view

![Full demo view with branch filters, curved commit history, and a colored diff inspector](docs/preview.png)

*The same demo history in the full view, with the branch list and commit inspector. Both previews use the application’s Ratatui layout and curve renderer with `--theme classic`. [Text-mode preview](docs/preview-text.svg).*

Open the full graph in a new tab to filter branches and inspect changes:

```bash
herdr plugin action invoke herdr.git-graph.open
```

To open a specific repository:

```bash
herdr plugin pane open \
  --plugin herdr.git-graph \
  --entrypoint graph \
  --cwd /path/to/repository \
  --focus
```

### Add a keybinding

Add an available key to `~/.config/herdr/config.toml`:

```toml
[[keys.command]]
key = "prefix+u"
type = "plugin_action"
command = "herdr.git-graph.open"
description = "Open Git Graph"

[[keys.command]]
key = "prefix+shift+u"
type = "plugin_action"
command = "herdr.git-graph.sidebar"
description = "Open Git Graph Sidebar"
```

`prefix+g` conflicts with Herdr's default `goto` action, and `prefix+shift+g` creates a worktree. Replace an existing graph binding on `prefix+g` with the example above. The `u` bindings are unused by Herdr defaults; choose others if your custom config already uses them. Run `herdr config check`, then `herdr server reload-config`. Press your prefix followed by `u` for the full view or `Shift+u` for the sidebar.

## Use

In the full view, select a branch and press **Enter** to show its history. Choose **All branches**, or press **a**, to return to the complete view. Select a commit to inspect its details below.

| Key | Action |
| --- | --- |
| `Tab` / `Shift-Tab` | Switch panels in the full view |
| `↑` `↓` / `j` `k` | Navigate items or scroll the diff |
| `Enter` | Apply search; in the full view, apply a branch filter / focus the inspector |
| `/`, then `n` / `N` | Search, then jump between matches |
| `a` | Show all branches |
| `r` | Reload local Git history |
| `d` | Toggle the inspector in the full view |
| `?` | Open / close scrollable Help |
| `q` / `Ctrl-C` | Quit; `q` closes Help first while it is open |

Mouse clicks select items; the wheel scrolls the panel under the pointer. See the [usage guide](docs/USAGE.md) for all shortcuts, rendering options, and troubleshooting.

The viewer reads local Git data and automatically reflects saved files, staging, commits, and branch changes. Press `r` for an immediate reload. To see new remote commits, fetch in your normal Git workflow; the viewer picks up the local update. It does not run fetch, checkout, commit, merge, rebase, or push. Search covers the loaded history: **2,000 commits by default**, configurable with `--limit`.

`Uncommitted changes` appears above HEAD in the all-refs view and the checked-out branch view, including before the first commit. The inspector separates staged and unstaged patches; untracked files are listed by name. Ignored untracked files are excluded. Background checks reuse history while refs and HEAD are unchanged, skip unchanged redraws, and back off for slow repositories. Use `--refresh-interval 5` to check less often or `--no-auto-refresh` for manual refresh only. See the [usage guide](docs/USAGE.md#uncommitted-changes-and-live-updates) for details.

## Run without Herdr

```bash
git clone https://github.com/sjlee06/herdr-git-graph.git
cd herdr-git-graph
sh scripts/install.sh

./bin/herdr-git-graph --demo --sidebar
./bin/herdr-git-graph --demo
./bin/herdr-git-graph --repo /path/to/repository
```

Standalone mode uses the Unicode graph. Pixel curves require a compatible Herdr pane and outer terminal. See [renderer selection](docs/USAGE.md#renderers) for details.

## Development

The [development guide](docs/DEVELOPMENT.md) covers local plugin linking, builds, tests, and the source layout. [Validation notes](docs/VALIDATION.md) distinguish automated checks from live terminal graphics testing.

Bug reports and contributions are welcome. Please include your OS, Herdr version, terminal, and the steps to reproduce the issue.

## License

[MIT](LICENSE). An independent community plugin for [Herdr](https://herdr.dev/).
