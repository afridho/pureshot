# capture-overlay

Local Windows capture dependency for PureShot, adapted from
[interactive-screenshot](https://github.com/stilltid/interactive-screenshot).
The upstream badges below refer to the original project.


[![crates.io](https://img.shields.io/crates/v/interactive-screenshot.svg)](https://crates.io/crates/interactive-screenshot)
[![docs.rs](https://img.shields.io/docsrs/interactive-screenshot)](https://docs.rs/interactive-screenshot)
[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](https://github.com/stilltid/interactive-screenshot)
[![MSRV](https://img.shields.io/badge/MSRV-1.75-blue)](https://gist.github.com/alopatindev/3c3650e2eab754b002f354e153c0bab0)
![Windows](https://img.shields.io/badge/platform-Windows-blue)

A native Windows screenshot capture library with interactive overlay selection. Provides a full-screen semi-transparent overlay with auto window detection, drag selection, and raw RGBA pixel output.

## Screenshots

![Drag region selection](https://raw.githubusercontent.com/stilltid/interactive-screenshot/master/assets/drag-selection.png)
*Drag to select any region on screen*

![Auto window detection](https://raw.githubusercontent.com/stilltid/interactive-screenshot/master/assets/window-detection.png)
*Hover over a window to auto-detect and highlight its border*

## Features

- **Full-screen capture** — captures the entire virtual desktop using Win32 GDI
- **Auto window detection** — highlights windows on mouse hover
- **Drag selection** — click and drag to select any region
- **DPI aware** — automatically handles high-DPI displays
- **Framework-free** — returns raw RGBA pixels, compatible with any UI framework
- **Cloaked window aware** — properly excludes virtual-desktop and background-suspended windows

## Installation

```toml
[dependencies]
capture-overlay = { path = "../vendor/capture-overlay" }
```

## Quick Start

### Interactive capture (with overlay)

```rust
use capture_overlay::{start_capture, CaptureOutcome};

fn main() {
    match start_capture() {
        Ok(CaptureOutcome::Captured { rgba, width, height }) => {
            // `rgba` contains the raw RGBA pixel data
            // `width` and `height` are the pixel dimensions
        }
        Ok(CaptureOutcome::Cancelled) => {
            // User right-clicked or pressed ESC
        }
        Err(e) => {
            // Handle the error
        }
    }
}
```

### Selector settings

```rust
use capture_overlay::{start_capture_with_options, CaptureOptions};

let outcome = start_capture_with_options(13, CaptureOptions {
    show_crosshair: true,
    show_magnifier: true,
    freeze_screen: false,
})?;
# Ok::<(), capture_overlay::CaptureError>(())
```

These are also the defaults used by `start_capture()` and
`start_capture_with_radius()`. Crosshair off uses an arrow and hides the full
pointer-aligned guides. The magnifier shows a 5× view of the clean desktop pixels,
with the center source pixel outlined. Neither aid appears in the returned image.

Live mode refreshes at approximately 30 FPS, including a final refresh at selection
completion, using `WDA_EXCLUDEFROMCAPTURE` to omit the selector itself. It requires
Windows 10 version 2004+ and DWM composition; older systems or failed exclusion
fall back to the startup snapshot and log a warning. Frozen mode always uses the
startup snapshot, including for the returned crop. Protected/DRM surfaces may
still be black, as with other GDI captures. Desktop dimensions are fixed for the
session; cancel and restart after monitor-layout changes. Call these blocking
APIs on a worker thread in GUI applications.

### Full-screen capture (programmatic)

```rust
use capture_overlay::{capture_full_screen, CaptureError};

fn main() -> Result<(), CaptureError> {
    let screen = capture_full_screen()?;
    // screen.rgba, screen.width, screen.height
    Ok(())
}
```

## Usage Guide

### Interactive capture

When you call `start_capture()`, the following happens:

1. **Capture screen** — captures the current desktop using Win32 GDI, stores as RGBA pixel data
2. **Enumerate windows** — lists all visible windows for auto-detection
3. **Show overlay** — creates a full-screen semi-transparent overlay window, always-on-top, displaying the captured desktop
4. **Wait for user input** — enters a message loop, handling mouse and keyboard events:

| Action | Behavior |
|--------|----------|
| **Mouse move** | Auto-detects the window under cursor, highlights its border |
| **Left-click + drag** | Starts manual region selection, updates in real-time |
| **Left release** | Confirms selection, crops the image, returns result |
| **Right-click** | Cancels capture, returns `Cancelled` |
| **ESC key** | Cancels capture, returns `Cancelled` |

5. **Cleanup** — destroys the overlay window, releases GDI resources, returns the result

### Programmatic full-screen capture

Call `capture_full_screen()` to capture the desktop directly without any overlay. Returns raw RGBA pixel data and screen dimensions.

## How It Works

- **Capture**: Uses `CreateCompatibleDC` + `BitBlt` to capture the desktop into a DIB section, then converts to RGBA format
- **Overlay**: Created with `CreateWindowExW` as a full-screen popup window with styles `WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_NOACTIVATE`, preventing focus stealing and mouse passthrough
- **Window detection**: Enumerates windows via `EnumWindows`, filters cloaked windows with `DWMWA_CLOAKED`, finds the window under cursor via `find_window_at_point`
- **Rendering**: Double-buffered (memory DC + `BitBlt`) to avoid flicker, `AlphaBlend` for semi-transparent mask
- **Crop**: Extracts the selected sub-region from the original RGBA data

## Output

The captured image is returned as raw RGBA pixels (`Vec<u8>`), 4 bytes per pixel (R, G, B, A).

## Logging

The library uses the `log` crate facade. The caller controls the backend:

| Level | Content |
|-------|---------|
| `info!` | Capture start/complete/cancel |
| `debug!` | Window enumeration, mouse coordinates, overlay lifecycle |
| `warn!` | DWM fallback, mask creation failure |

All logs are silent when no logger backend is registered.

## Platform Support

| Platform | Status |
|----------|--------|
| Windows 7+ | ✅ Full support |
| Windows 10+ | ✅ Full support (includes DWMWA_CLOAKED handling) |
| Linux / macOS | ❌ Not supported |

## Safety

This crate necessarily uses `unsafe` for Win32 FFI calls. All `unsafe` blocks are documented with `// SAFETY:` comments explaining why the call is sound. The API surface is fully safe to call from Rust.

## Requirements

- Windows 7+
- Rust edition 2021+

## Changelog

See [CHANGELOG.md](CHANGELOG.md) for release history.

## License

Licensed under either of:

- MIT license ([LICENSE-MIT](LICENSE-MIT))
- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))

at your option.
