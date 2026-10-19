# PureShot

PureShot is a screenshot utility for Windows and Linux, built with Tauri 2
(React + TypeScript frontend, Rust backend). Windows uses native Win32 capture
and floating windows; Linux uses the desktop Screenshot Portal, WebView
quick previews, and native GTK floating pins. Native area selection on GNOME Wayland uses a companion
GNOME Shell extension with a modal drag overlay on the live desktop, without
freezing it or opening the GNOME screenshot toolbar.
See [LINUX.md](LINUX.md) for setup and Wayland limitations.

Out of scope for this project: screen recording, a video editor, and any
cloud sync/sharing feature.

## Features

- **Full-screen & area capture** — native area selection (crosshair, window
  highlight on hover, multi-monitor and mixed-DPI support).
- **Quick Access Overlay** — a separate borderless desktop window that
  appears after a capture, with Pin, Close, Annotate, and Save/Copy controls
  that show on hover, a configurable auto-close duration, and an animated
  slide-out on close.
- **Pin / Floating Screenshot** — a native Win32 window you can move around,
  always on top, with:
  - pinch-to-zoom and two-finger scroll for opacity on precision touchpads
  - an anti-aliased drop shadow and rounded corners (software-composited via
    `UpdateLayeredWindow`, not plain GDI)
  - a smooth close button that responds to hover
- **Dashboard & history** — capture history, a tray icon, and settings
  (including a corner radius that applies to the overlay, floating window,
  and previews).
- Captures are cached locally; Save exports to the configured folder (by default
  `Pictures/PureShot`, using the desktop's Pictures directory).

## Capture OCR

PureShot uses [ocrs](https://github.com/robertknight/ocrs) (MIT/Apache-2.0)
with the Rust RTen inference engine. No Python, OCR server, or API key is needed.
QR decoding uses the pure-Rust `quircs` decoder and needs no models or network access.
The selected region is checked for QR codes first; decoded payloads are copied exactly
(multiple distinct codes are separated by newlines). Links are never opened automatically.
If no QR code is decoded, recognition falls back to OCR, which requires the models below.

1. Open **Settings → Screenshots → OCR** and download the
   models. This explicit setup step requires internet access and downloads only
   the detection/recognition models from the upstream ocrs model host.
2. Choose **Capture OCR** from the tray or settings,
   and select an area. On GNOME this uses the existing companion extension.
3. Recognition runs in the background, copies the text automatically, and shows
   a desktop notification with a text preview. No editor or extra Copy click is
   required. You can also extract text from History (and the Linux preview).
   If models are missing, History opens the setup panel, where QR decoding still works.

Models are kept in the application's local data directory under `ocr-models`.
After setup, recognition runs offline; screenshots are never uploaded. OCR
captures retain square corners to avoid clipping text and do not automatically
replace the clipboard with an image. Empty results leave the clipboard unchanged.
The upstream engine is an early preview and recognizes Latin script only;
Indonesia/English screenshot accuracy must be checked on real captures. Other
scripts are not supported by this engine. Assign the OCR action in **Settings → Shortcuts**
to capture text or QR codes using the same shortcut. On GNOME Wayland this uses the
existing companion extension; no extension update is needed for QR decoding.

## Tech stack

- [Tauri 2](https://tauri.app/) — desktop app shell
- React 19 + TypeScript — frontend (dashboard, settings, overlay routing)
- Rust — backend and Tauri commands
- `windows` crate (Win32 API) — native windows for the selection overlay and
  pinned screenshot, including layered window compositing and touchpad
  gesture handling

## Project structure

```
src/                  React frontend (dashboard, settings, overlay, history)
src/tauri.ts          Frontend calls into Tauri commands
src-tauri/src/lib.rs  Tauri commands, tray menu, global shortcuts, window lifecycle
src-tauri/src/native_pin.rs
                      Native Win32 window for the pinned floating screenshot
```

See `AGENTS.md` for project conventions, and
`.opencode/skills/pureshot-native-win32/SKILL.md` for technical notes on the
native Win32 code (layered window compositing, touchpad gestures, and
`windows` crate binding quirks).

## Running locally

Prerequisites: Node.js, the Rust toolchain, and the Tauri prerequisites for
your platform (see the [Tauri prerequisites guide](https://tauri.app/start/prerequisites/)).
Linux also needs `xdg-desktop-portal` and a screenshot-capable desktop backend.

```bash
npm install
npm run tauri dev
```

Release build:

```bash
npm run tauri build
```

## Status

This project is under active development. Capture, preview, pin, annotation,
clipboard, and local export are implemented. Platform-specific behavior and
Linux smoke-test instructions are documented in [LINUX.md](LINUX.md).
