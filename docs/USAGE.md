# Usage guide

[Back to README](../README.md) · [한국어 소개](../README.ko.md)

## Panels

**Branches** lists local and remote-tracking branches. Press Enter to show the commits reachable from the selected branch. Press `a` to include all local refs and the current HEAD again. Tags appear alongside commits in History.

**History** displays the graph node, hash, subject, ref labels, and, when space permits, author and date. Search jumps between matching commits while preserving the full graph topology of the loaded view.

**Commit inspector** shows metadata, changed-file statistics, and a patch. Merge commits are compared with their first parent. Focus this panel to scroll the diff independently.

The full view's minimum pane size is 64 columns × 18 rows. Around 120 columns or more gives the history more room. In narrower panes, the branch list is hidden until you focus it with Tab.

## Graph-only sidebar

```bash
# Open beside the current Herdr pane, keeping the current focus
herdr plugin action invoke herdr.git-graph.sidebar

# Open the registered sidebar explicitly
herdr plugin pane open --plugin herdr.git-graph --entrypoint sidebar \
  --placement split --direction right --target-pane "$HERDR_PANE_ID" \
  --cwd /path/to/repository --no-focus

# Use graph-only layout in an existing terminal pane, or preview it
./bin/herdr-git-graph --repo /path/to/repository --sidebar
./bin/herdr-git-graph --demo --sidebar --width 40 --height 24 --snapshot sidebar.svg
```

The sidebar displays only the history graph and commit list, with repository and HEAD context above it. Commit subjects occupy the first line; hashes and ref labels occupy the second. The branch list, author/date columns, and diff inspector are hidden. Commit diffs are not loaded in this mode.

The minimum size is 24 columns × 8 rows. Adjust the split width by dragging the divider or using Herdr's resize mode (`prefix+r` by default). Focus it with `prefix+l` from the pane on its left. Search, `n`/`N`, navigation, graph panning, mouse selection/scrolling, `r`, help, and quit work as usual. `Tab`, `Shift-Tab`, `Enter` outside search, and `d` keep the graph-only layout. Close the sidebar with `q` while it is focused. Each open action creates a new split.

The action takes its target from the invocation's focused pane. When opening the pane explicitly, run the command above inside Herdr, or pass a known pane ID with `--target-pane`. Split panes must not use `--workspace`.

Suggested bindings are `prefix+u` for `herdr.git-graph.open` and `prefix+shift+u` for `herdr.git-graph.sidebar`; see the [README config example](../README.md#add-a-keybinding). Avoid `prefix+g` (`goto`) and `prefix+shift+g` (new worktree), which Herdr already binds by default. Validate with `herdr config check` before reloading the configuration.

## Keyboard and mouse

| Input | Action |
| --- | --- |
| `Tab` / `Shift-Tab` | Next / previous panel |
| `↑` `↓` / `j` `k` | Navigate items or scroll the inspector |
| `Enter` in Branches | Apply the selected branch filter |
| `Enter` in History | Open and focus the inspector |
| `PageUp` / `PageDown` | Move one page |
| `Home` / `End`, or `g` / `G` | First / last item |
| `←` `→` / `h` `l` in History | Pan graph columns horizontally |
| `←` `→` / `h` `l` in Inspector | Scroll diff text horizontally |
| `/` | Enter search mode |
| `Enter` while searching | Keep the query and return to navigation |
| `n` / `N` | Next / previous match, wrapping at the ends |
| `Esc` | Clear search and focus History; close Help if open |
| `a` | Load all branches |
| `r` | Reload the current local history view |
| `d` | Show / hide the inspector |
| `?` | Open / close Help |
| `q` / `Ctrl-C` | Quit; `q` closes Help first if it is open |
| Left click | Select a commit or apply a branch filter |
| Mouse wheel | Scroll the panel under the pointer |

## Renderers

```bash
./bin/herdr-git-graph --renderer auto
./bin/herdr-git-graph --renderer text
./bin/herdr-git-graph --renderer curves
```

- **auto** is the default. It attempts Herdr graphics when the required pane environment is available and otherwise uses text.
- **text** always uses colored Unicode nodes and connecting lines.
- **curves** attempts the same pixel renderer, with a visible fallback reason if setup fails.

Smooth curves use tiny-skia to draw antialiased PNG frames in the graph area. Ratatui renders the surrounding text and panels. Herdr's `pane.graphics.info` supplies cell pixel dimensions, and a dedicated `git-graph` stream layer displays the images. Unchanged graph frames are not retransmitted; closing the stream removes its layer.

Herdr 0.9.0 enables `[terminal].kitty_graphics` by default. If it is disabled, or the terminal cannot report usable pixel dimensions, the viewer falls back to text. Connection and frame-write failures also trigger a fallback. Pixel rendering depends on the outer terminal and Herdr version; `--renderer curves` alone does not enable graphics outside Herdr.

See [validation notes](VALIDATION.md) for the distinction between automated renderer tests and live terminal testing.

## Repository selection and limits

An explicit `--repo /path/to/repository` takes precedence. A plugin action passes its selected repository to the new pane. Otherwise the app checks the working directory and available Herdr context, including the source pane directory, linked worktree checkout, and workspace directory.

Detached HEAD commits are included in the all-refs view. A shallow clone only shows history available locally.

```bash
./bin/herdr-git-graph --repo /path/to/repository --limit 10000
```

The default limit is 2,000 commits; accepted values range from 1 to 50,000. Search only covers the loaded commits. The footer indicates when a limit is reached. Wide graphs can be panned with `h` / `l`.

Press `r` to reread local refs and history. Obtain new remote commits with your existing Git workflow, then reload the viewer. This version does not provide Git writes, GitHub PR data, automatic fetch, or filesystem watching.

Git subprocesses have a 15-second timeout, and diff previews are capped at 512 KiB. Pagers, external diff programs, and textconv are disabled. Repository reloads and commit detail reads run in a background worker; the first load occurs before the TUI opens.

## Troubleshooting

| Symptom | What to check |
| --- | --- |
| Installation cannot find `cargo` | Reinstall from GitHub to get v0.1.1 or later; release installs no longer need Rust. Only explicit source builds require Cargo. |
| Release download fails | Check access to GitHub Releases and retry. No existing binary is replaced on a failed download or checksum mismatch. |
| Release cannot run on this OS | Prebuilt binaries need macOS 11+ or Linux glibc 2.35+, on arm64/x86_64. See source builds for other environments. |
| Plugin action is missing | Check `herdr plugin list` and `herdr plugin action list --plugin herdr.git-graph`. |
| Sidebar action says `running` but no pane appears | This response only acknowledges launch. Inspect `herdr plugin log list --plugin herdr.git-graph`. Version 0.2.0 incorrectly passed `--workspace` for a split; reinstall to get 0.2.1 or later. |
| The configured key does nothing | Run the action directly, check for a conflicting binding, and reload the active session's configuration. |
| The folder is not a Git repository | Open a Git workspace or pass `--repo` / `--cwd` explicitly. |
| No commits appear | An empty repository needs its first commit. Then press `r`. |
| A commit cannot be found | Clear the branch filter with `a`, fetch if needed, or increase `--limit`. |
| Curves are unavailable | Run inside Herdr, check the outer terminal and graphics setting, or use `--renderer text`. |
| A pane is too small | Resize it; the full view needs 64 × 18, and `--sidebar` needs 24 × 8. |

For local plugin development, rebuild before linking. GitHub installs run the manifest build command; `herdr plugin link` does not.
