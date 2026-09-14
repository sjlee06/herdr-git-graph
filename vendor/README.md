# Crossterm terminal disconnect patch

`crossterm/` contains the source, examples, manifest, README and MIT license from
the published `crossterm` 0.29.0 crate. Other upstream documentation and development
files are omitted. The examples are retained because the manifest lists them.
Upstream line endings and whitespace are preserved; `.gitattributes` marks this
directory as vendored and excludes its original formatting from whitespace checks.

- Upstream: https://github.com/crossterm-rs/crossterm
- Published source: https://docs.rs/crate/crossterm/0.29.0/source/
- Release commit: `36d95b26a26e64b0f8c12edfe11f410a6d56a812`
- Crate SHA-256: `d8b9f2e4c67f833b660cdb0a3523065869fb35570177239812ed4c905aeff87b`

The behavioral changes are confined to `src/event/source/unix/mio.rs`:

1. A zero-byte terminal read returns `UnexpectedEof` instead of retrying forever.
2. Read errors other than `WouldBlock` and `Interrupted` are returned instead of
   retried forever.

An unnecessary pair of parentheses in `src/terminal/sys/unix.rs` is also removed
to keep the local dependency free of the upstream `unused_parens` warning.

Without this patch, closing a PTY can leave a process consuming an entire CPU
core inside `event::poll`, even after SIGTERM/SIGHUP set the app's stop flag.
The application handles EOF as normal shutdown; other errors still unwind and
drop the terminal guard and graphics stream.

The root `[patch.crates-io]` applies the same implementation to all users of
Crossterm 0.29, including Ratatui. Do not patch the machine's Cargo registry cache.
Remove this override when a released upstream version handles both cases, after
running `python3 tests/smoke.py ./bin/herdr-git-graph --work /tmp/hgg-smoke`.
