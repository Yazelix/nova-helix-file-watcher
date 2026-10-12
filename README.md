# Nova Helix file watcher

Forked from [mattwparas/helix-file-watcher](https://github.com/mattwparas/helix-file-watcher) at `8cd0726da47be4a1011c3246ff308c1dfefda9d1` (MIT). The fork preserves unsaved Helix buffers when their files change on disk and pins Steel to the revision used by Nova Helix. Nova can remove this fork when native Helix file watching covers the same behavior.

Clean open documents reload after external writes, including atomic file replacements and writes following Helix's own atomic save. Dirty buffers retain their edits and receive a warning. Rust owns nonrecursive parent-directory subscriptions shared by open files and filters sibling activity before sending events to Steel. Open, save and close hooks reconcile the subscriptions; registration failures produce a visible warning.

Open files retain their subscriptions during temporary absence inside an existing directory. Canonical notification paths resolve to documents opened through symlinks. Reload failures are logged per file so a deleted file does not interrupt other pending reloads.

This covers replacement of files inside an existing directory. Replacing or removing the watched parent directory and recovery from native notification backend failures are not covered. Native macOS behavior needs its own runtime verification.

This is a helix plugin made using steel. To install, you can use the `forge` command line tool,
which also requires having a rust toolchain installed.

You can either clone the repo and then from the root run:

`forge install`

Or you can do:

`forge pkg install --git https://github.com/Yazelix/nova-helix-file-watcher.git`.

This will build and install the library.

You should then be able to use the library like so:

```steel
(require "nova-helix-file-watcher/file-watcher.scm")
```

To watch open files, including files opened later:

```scheme
(spawn-watcher)
```

Verify with `cargo fmt --check`, `cargo test --locked` and `cargo build --locked --release --lib`. The native filesystem regression exercises replacement, later writes, shared-directory cleanup, sibling filtering and registration failure/retry. The Steel engine test checks reload safety, lifecycle hooks, registration warnings, symlink aliases, temporary file absence and deletion before delayed reload.

LOC scorecard: **624 code lines** across Rust and Scheme, including tests, measured with `tokei src tests file-watcher.scm helix-file-watcher.scm`. The subscription owner and filesystem/lifecycle regressions account for the growth; no dependency is added.
