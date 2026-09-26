# Nova Helix file watcher

Forked from [mattwparas/helix-file-watcher](https://github.com/mattwparas/helix-file-watcher) at `8cd0726da47be4a1011c3246ff308c1dfefda9d1` (MIT). The fork preserves unsaved Helix buffers when their files change on disk and pins Steel to the revision used by Nova Helix. Nova can remove this fork when native Helix file watching covers the same behavior.

This is a helix plugin made using steel. To install, you can use the `forge` command line tool,
which also requires having a rust toolchain installed.

You can either clone the repo and then from the root run:

`forge install`

Or you can do:

`forge pkg install --git https://github.com/Yazelix/nova-helix-file-watcher.git`.

This will build and install the library.

You should then be able to use the library like so:

```steel
(require "helix-file-watcher/file-watcher.scm")
```

To start the watcher on the current directory:

```scheme
(spawn-watcher)
```
