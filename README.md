# StorageScope

TreeSize-like terminal disk usage analyzer.

## Features

- Incremental scan updates while traversal is running.
- Directory-first table with sortable sizes and relative bars.
- Drill-down navigation and parent navigation.
- Size metric toggle: allocated vs apparent.
- Hidden entries included by default.
- Directory rows by default for better performance on huge trees (`--show-files true` to include files).
- Same-filesystem traversal by default.
- Navigation reuses in-memory scan data instead of rescanning on every folder change.
- Live loading indicators in table/footer while scan results are still streaming in.
- Guarded delete flow (`DELETE` typed confirmation).

## Run

```bash
cargo run -- .
```

## CLI

```bash
storagescope [PATH] [--one-file-system true|false] [--follow-symlinks true|false] [--show-hidden true|false] [--show-files true|false] [--metric allocated|apparent] [--max-depth N] [--no-delete]
```

For very large scans (`/`, large home dirs), keep `--show-files false` and optionally cap traversal with `--max-depth`.

## Keybindings

- `j` / `k` or arrows: move selection
- `Enter`: drill into selected directory
- `h` / `Backspace`: go to parent
- `s`: cycle sort mode
- `m`: toggle metric
- `r`: rescan current path
- `/`: type filter
- `?` / `F1`: open help modal
- `d`: delete selected entry (unless `--no-delete`)
- `q`: quit
