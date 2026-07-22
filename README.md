# Orienteering map app

A cross-platform mobile app for orienteering: photograph a paper orienteering map,
get the terrain/course automatically recognized (ISOM terrain classes, controls,
start/finish, course legs), correct it by hand where recognition gets it wrong, and
find an offline least-cost route between controls — no network connection needed in
the field.

Full project spec (roles, architecture, fixed tech stack, phase plan) is in
[`prompt.txt`](prompt.txt) (Russian) — read it before doing any non-trivial work here,
since it defines constraints the rest of this repo must not violate.

## Status

- **Phase 0 (done, frozen):** Python CV/path-finding prototype in
  [`python_prototype/`](python_prototype/), proving the pipeline shape
  (photo → rectify → segment terrain/course → cost-grid → least-cost path) works end
  to end on real photographed maps. See [`PHASE0_HANDOFF.md`](PHASE0_HANDOFF.md) for
  what carries forward into Phase 1, what's photo-specific calibration that must not
  be copied as-is, and what was already tried and abandoned.
- **Phase 1 (in progress):** Flutter client + Rust core in [`app/`](app/) — the
  production codebase (not a port of the Python prototype). Perspective correction,
  classical CV segmentation, course autodetection, manual correction, and Dijkstra
  path-finding are implemented and verified on Android (real device) and Windows
  desktop.

## Repository layout

| Path | What it is |
|---|---|
| [`prompt.txt`](prompt.txt) | The original project spec — read first. |
| [`PHASE0_HANDOFF.md`](PHASE0_HANDOFF.md) | Condensed Phase 0 → Phase 1 handoff manifest. |
| [`CLAUDE.md`](CLAUDE.md) | Detailed, continuously-updated dev log: environment setup, every non-obvious bug hit and how it was fixed, architecture notes for both subprojects. The most complete source of truth for "why is it built this way" — check it before re-deriving something from scratch. |
| [`python_prototype/`](python_prototype/) | Phase 0: Python CV/path-finding prototype. Complete, frozen, not shipped. |
| [`app/`](app/) | Phase 1: Flutter + Rust production app. Active development. |

## Getting started

- **Flutter + Rust app:** see [`app/README.md`](app/README.md).
- **Python prototype:** see [`python_prototype/README.md`](python_prototype/README.md).
- Either way, skim [`CLAUDE.md`](CLAUDE.md) first if you're setting up this repo on a
  new machine — it documents every toolchain gotcha (OpenCV/NDK env vars, cargokit
  patches, etc.) already hit and fixed once.
