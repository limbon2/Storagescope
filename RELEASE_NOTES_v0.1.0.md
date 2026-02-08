# StorageScope v0.1.0

First public release of StorageScope, a TreeSize-like terminal disk usage analyzer.

## Highlights

- Fast incremental scanning with live updates while traversal runs.
- Directory-first UI optimized for large trees.
- Relative usage bars with improved gradient rendering.
- Keyboard + mouse navigation.
- Column toggles (`Shift+N/K/S/R/P`) with clear header hints.
- Disk capacity header (`total`, `used`, `free`).
- Live loading indicators for large scans.
- Omarchy-aware theming with live theme reload while the app is running.
- Safe delete workflow with typed `DELETE` confirmation.

## Usability & Performance

- Reduced UI lag on very large directories.
- Better selection scrolling/viewport behavior.
- Improved loading feedback for long scans.
- Smoother scan event handling and faster top-level discovery.

## Project/Docs

- Added `README.md` screenshot + updated docs.
- Added `CONTRIBUTING.md` including AI-assisted contribution policy.
- Added `RELEASING.md` release checklist.
- Added `LICENSE` (MIT).

## Notes

- For very large trees, prefer `--show-files false` and optionally set `--max-depth`.
