# Usage guide

[Back to README](../README.md) · [한국어 소개](../README.ko.md)

## Panels

**Branches** lists local and remote-tracking branches. Press Enter to show the commits reachable from the selected branch. Press `a` to include all local refs and the current HEAD again. Tags appear alongside commits in History.

**History** displays the graph node, hash, subject, ref labels, and, when space permits, author and date. Search jumps between matching commits while preserving the full graph topology of the loaded view.

**Commit inspector** shows metadata, changed-file statistics, and a patch. Merge commits are compared with their first parent. Focus this panel to scroll the diff independently.

The minimum pane size is 64 columns × 18 rows. Around 120 columns or more gives the history more room. In narrower panes, the branch list is hidden until you focus it with Tab.

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
| Installation cannot find `cargo` | Install Rust stable and ensure Cargo is on the `PATH` used by Herdr. |
| Plugin action is missing | Check `herdr plugin list` and `herdr plugin action list --plugin herdr.git-graph`. |
| The configured key does nothing | Run the action directly, check for a conflicting binding, and reload the active session's configuration. |
| The folder is not a Git repository | Open a Git workspace or pass `--repo` / `--cwd` explicitly. |
| No commits appear | An empty repository needs its first commit. Then press `r`. |
| A commit cannot be found | Clear the branch filter with `a`, fetch if needed, or increase `--limit`. |
| Curves are unavailable | Run inside Herdr, check the outer terminal and graphics setting, or use `--renderer text`. |
| A pane is too small | Resize it; the minimum is 64 × 18. |

For local plugin development, rebuild before linking. GitHub installs run the manifest build command; `herdr plugin link` does not.
