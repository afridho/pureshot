# PureShot — Agent Instructions

PureShot is a Windows screenshot utility similar to CleanShot X, built with
Tauri 2 (React + TypeScript frontend, Rust backend) plus native Win32 code for
anything the WebView cannot do reliably: screen capture, selection overlays,
and the pinned floating screenshot window.

Explicitly out of scope: screen recording, a video editor, and any cloud
sync/sharing feature. Do not add these unless the user asks and confirms the
scope change.

## Repo layout

- `pureshot/` — the actual app (Tauri 2). This is the primary project.
  - `src/` — React frontend (dashboard, settings, history, quick-access
    overlay routing). Entry: `src/App.tsx`. Capture/action calls go through
    `src/tauri.ts`.
  - `src-tauri/src/lib.rs` — Tauri commands, tray menu, global shortcuts,
    window lifecycle (`capture_screen`, `start_area_capture`,
    `show_capture_overlay`, `pin_capture`, `capture_action`).
  - `src-tauri/src/native_pin.rs` — native Win32 window for the pinned
    floating screenshot (see the `pureshot-native-win32` skill before editing
    this file).
  - `src-tauri/Cargo.toml` — Rust dependencies; `windows` crate features are
    listed explicitly under `[target.'cfg(windows)'.dependencies]`.
- `vendor/capture-overlay/` — standalone Rust crate for the native area/window
  selection overlay (crosshair, window hover highlight, DPI handling, rounded
  selection border). `src/native_overlay.rs` is the overlay window procedure;
  `src/lib.rs` exposes `start_capture_with_radius`.
- `Clippity/` and `quickshotter/` — separate/earlier projects in this same
  parent directory. Do not assume shared code or shared intent with
  `pureshot/` unless a task explicitly references them. `Clippity` has been
  used only as a reference for hidden/floating window lifecycle patterns.

`pureshot/` (this directory) is its own git repository, pushed to
`git@github.com:afridho/pureshot.git` on branch `master`. The parent
directory (`/mnt/c/dev/pureshot`) is not a repository; `Clippity/` and
`quickshotter/` are not tracked here and should not be assumed to share
history or remotes with this repo.

## Before making changes

- Read the relevant file fully before editing. `lib.rs` and `native_pin.rs`
  are dense; small context windows miss adjacent state that another change
  depends on.
- Settings like corner radius live in the frontend's `localStorage` and are
  passed into Rust commands as parameters (e.g. `cornerRadius`,
  `corner_radius`). There is no persisted Rust-side settings store yet.
- `CaptureItem` (id, path, data_url, width, height, created_at) is the shared
  shape between Rust and TypeScript (`src/types.ts`). Keep both sides in sync
  if it changes.

## Rust / Tauri conventions

- Target platform is Windows; non-Windows builds exist mostly as a fallback
  (`#[cfg(not(windows))]` branches) and are not the focus of testing.
- Blocking native Win32 message loops (area selection, the pinned window) must
  run on a spawned `std::thread`, never on Tauri's UI thread.
- When hiding the main WebView window before a capture, wait briefly
  (~280ms) before capturing. WebView2's `hide()` returns before DWM commits
  the new frame; capturing immediately bakes the dashboard into the
  screenshot.
- Prefer reusing an existing plugin (`tauri-plugin-shell`, etc.) or a plain
  `std::process::Command` over adding a new dependency for a one-off feature
  (e.g. opening the capture folder uses `explorer.exe` directly instead of
  pulling in `tauri-plugin-opener`).
- Pin an exact `windows` crate version in `Cargo.toml` features rather than
  widening the feature set speculatively; only add a feature when a specific
  API requires it, and verify the binding exists first (see the
  `pureshot-native-win32` skill for how to check quickly).

## Verification

- Do not run `cargo check` or `cargo build` for the `pureshot/src-tauri`
  crate unless the user explicitly asks, or the change is significant enough
  to need milestone validation. The user runs `npm run tauri dev` themselves
  and watches the terminal; running a competing cargo process at the same
  time causes file-lock conflicts and confusing interleaved output (this has
  happened before — see the `pureshot-native-win32` skill for the isolated
  compile-check workaround that avoids touching their running dev server).
- Frontend type-check: `./node_modules/.bin/tsc --noEmit` from `pureshot/`
  works and should be used after non-trivial TypeScript changes.
- `npm run build` currently fails in this environment due to a missing
  optional dependency (`@rollup/rollup-linux-x64-gnu`), unrelated to app
  code. Don't treat that failure as a regression you introduced.
- After editing Rust files, if you need confidence beyond reading the code,
  use the isolated single-file compile-check approach from the
  `pureshot-native-win32` skill instead of building the whole Tauri app.

## Restart instructions to give the user

After native (Rust) changes, the dev server needs a manual restart (hot
reload does not apply to native windows/processes already created):

```powershell
Get-Process pureshot,rustc -ErrorAction SilentlyContinue | Stop-Process -Force
npm run tauri dev
```

## Skills

- `pureshot-native-win32` — load this before touching `native_pin.rs`,
  `vendor/capture-overlay/src/native_overlay.rs`, or any new native Win32
  window/surface. It documents the layered-window compositing approach,
  touchpad gesture classification, and `windows` crate binding quirks learned
  while building the pin feature.
- `pureshot-icons` — load this before adding, replacing, styling, or
  rasterizing icons in the React UI or native Win32 layered windows. It covers
  the Lucide source of truth, embedded native assets, supersampled downscaling,
  DPI handling, hover states, and premultiplied-alpha compositing.
- `pureshot-theming` — load this before changing light/dark theme behavior,
  colors, surfaces, controls, or text contrast in the React/WebView settings
  UI. It documents Windows theme following, CSS cascade pitfalls, contrast
  targets, required states, and the full visual verification checklist.
