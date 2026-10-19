---
name: PureShot Native Win32 (Tauri + windows crate)
description: Use when writing or editing native Win32 window code in the PureShot project — the pinned floating screenshot (native_pin.rs), the native selection overlay (capture-overlay), or any new layered/borderless Win32 window driven from Rust/Tauri. Covers layered-window compositing for shadows and anti-aliasing, touchpad pinch/scroll gesture classification, and windows crate 0.62 binding quirks.
---

## When this applies

PureShot replaces several things a Tauri WebView window cannot do well with
plain Win32 windows owned directly by Rust:

- the pinned floating screenshot (`pureshot/src-tauri/src/native_pin.rs`)
- the area/window selection overlay
  (`vendor/capture-overlay/src/native_overlay.rs`)

Load this skill before editing either file, or before adding a new native
window of the same kind (anything borderless, always-on-top, or needing
touchpad gestures).

## Why native windows instead of WebView

WebView2 renders in a separate process (`Chrome_RenderWidgetHostHWND`). You
cannot `SetWindowSubclass` it, intercept `WM_POINTER` on it, or get reliable
per-pixel alpha control from the Rust side. If a feature needs raw pointer
input, custom hit-testing, or a software-composited translucent surface, it
needs to own its own `HWND` — don't try to force it through the WebView.

## Layered window compositing (shadows, anti-aliasing, rounded corners)

`SetWindowRgn` + `SetLayeredWindowAttributes` (uniform alpha) cannot produce a
soft shadow or anti-aliased edges — region clipping is hard-edged, and
`LWA_ALPHA` applies one alpha value to the whole window including a shadow
margin, so the margin ends up fully opaque instead of fading out.

The approach that works: composite the whole window yourself into a
premultiplied 32-bit BGRA buffer in Rust, then push it with
`UpdateLayeredWindow` (`ULW_ALPHA`). This is what `native_pin.rs` does. Key
pieces:

- A `CreateDIBSection` bitmap selected into a `CreateCompatibleDC`, resized
  only when the window's content size actually changes (`ensure_surface`).
- Signed-distance-field helpers (`round_box_sdf`, `dist_seg`) to anti-alias
  rounded rectangles and line strokes with a single sample per pixel instead
  of supersampling — this is fast enough to re-render every frame during a
  live resize (pinch-zoom).
- A drop shadow is just the same rounded-rect SDF, offset down a few pixels,
  with alpha falling off over a blur distance (`t * t * SHADOW_STRENGTH`).
  Leave transparent margin around the content (`MARGIN` in `native_pin.rs`)
  for the shadow to live in.
- Opacity is applied as a multiplier in the compositor (`op = opacity / 100`)
  on every channel including alpha — not through `SetLayeredWindowAttributes`,
  which only existed in an earlier, cruder version of this window.
- Performance: add fast paths that skip the SDF math entirely for pixels
  deep inside the content rect (just copy the scaled image) and for pixels
  far outside the shadow's reach (just write transparent black). Only the
  border/corner/button region needs the expensive per-pixel math.

Don't reach for GDI+ (`Gdiplus`) for this — it's not worth the added linking
complexity. Pure-Rust SDF compositing is simpler to get right and to debug.

## Hover affordances: use TrackMouseEvent, not a timer

To show/hide a control (like a close button) on hover and hide it exactly
when the cursor leaves the window, use the real API:

```rust
WM_MOUSEMOVE => {
    // show the control if not already hovered
    let mut tracking = TRACKMOUSEEVENT {
        cbSize: mem::size_of::<TRACKMOUSEEVENT>() as u32,
        dwFlags: TME_LEAVE,
        hwndTrack: hwnd,
        dwHoverTime: 0,
    };
    let _ = TrackMouseEvent(&mut tracking);
}
WM_MOUSELEAVE => {
    // hide the control
}
```

`TrackMouseEvent` and `TRACKMOUSEEVENT`/`TME_LEAVE` live in
`Win32::UI::Input::KeyboardAndMouse` in the `windows` crate — easy to miss
since most window-message plumbing comes from `WindowsAndMessaging`.

`WM_MOUSELEAVE` itself is **not** exported from `WindowsAndMessaging` in
`windows 0.62` — it's behind the `Win32_UI_Controls` feature instead. Rather
than add that feature for one constant, define it locally:

```rust
const WM_MOUSELEAVE: u32 = 0x02A3;
```

A `SetTimer`-based "hide after N ms of no movement" is a trap: it fires late,
feels like lag, and still shows the control for a while after the cursor is
already gone. Only fall back to it if `TrackMouseEvent` is somehow
unavailable, which has not been the case so far.

## Touchpad gesture classification (pinch-zoom vs two-finger scroll/opacity)

Precision Touchpad gestures arrive as `WM_POINTER` messages once the window
calls `RegisterTouchpadCapableWindow` (loaded dynamically via
`GetProcAddress` on `user32.dll` — it's Windows 11+ only and not in the
`windows` crate's static bindings). Read both contacts' himetric positions
with `GetPointerFrameInfoHistory`.

The failure mode to avoid: classifying the gesture once at gesture start
(e.g. "if distance changed >0.8% from the start, it's a pinch") almost always
misclassifies a two-finger scroll as a pinch, because finger separation drifts
slightly during any real scroll. The fix is to classify **per frame**,
comparing two deltas computed from the *previous* frame, not the gesture's
start:

```rust
let separation_change = (distance - last_distance).abs();
let translation = (center_y - last_y).abs();
if separation_change > translation {
    // pinch → resize
} else if translation > 1.0 {
    // two-finger vertical scroll → adjust opacity
}
```

Whichever delta is bigger that frame wins. This lets a user mix pinching and
scrolling naturally in one continuous gesture without one gesture type
"locking out" the other.

## `windows` crate 0.62 binding quirks (checked against this exact version)

These are not guesses — they were confirmed by reading
`~/.cargo/registry/.../windows-0.62.2/src/Windows/Win32/**/mod.rs` directly
when something didn't compile. Do the same instead of assuming an API shape
from older `windows` crate versions or from C++ documentation:

- `SelectObject` / `DeleteObject` take `HGDIOBJ`, not `HPEN`/`HBRUSH`/`HBITMAP`
  directly. Convert explicitly: `SelectObject(dc, pen.into())`,
  `DeleteObject(brush.into())`.
- Many functions that used to return `BOOL` now return `windows_core::Result<()>`
  (e.g. `TrackMouseEvent`, `KillTimer`, `SetLayeredWindowAttributes`,
  `UpdateLayeredWindow`). Use `let _ = the_call(...)` or `.ok()?` instead of
  comparing to a boolean.
- `EnumChildWindows`, `GetMessageW`, and friends take `Option<HWND>` for
  "no window" / "any window", not a zeroed `HWND`.
- `InvalidateRect` and `SetWindowRgn` live in `Win32::Graphics::Gdi`, not
  `Win32::UI::WindowsAndMessaging` — easy to misplace since most window
  message constants/functions come from the latter.
- `VK_ESCAPE` and `ReleaseCapture` live in
  `Win32::UI::Input::KeyboardAndMouse`.
- `GetPointerFrameInfoHistory` and `POINTER_INFO` need the
  `Win32_UI_Input_Pointer` feature; `POINTER_INFO.pointerType` compares against
  `PT_TOUCHPAD`, which is in `WindowsAndMessaging`.
- `RegisterTouchpadCapableWindow` / `RegisterTouchpadCapableThread` are not in
  the crate's static bindings at all (Windows 11+ API) — load them with
  `GetModuleHandleW(w!("user32.dll"))` + `GetProcAddress` +
  `mem::transmute` into a typed `unsafe extern "system" fn` pointer.

When a new API doesn't compile, grep the actual installed crate source for
the real signature before guessing again:

```sh
grep -n "pub unsafe fn TheFunctionName" \
  ~/.cargo/registry/src/*/windows-0.62.*/src/Windows/Win32/**/mod.rs
```

## Verifying Rust changes without disrupting the user's `tauri dev`

The user runs `npm run tauri dev` themselves in their own terminal and watches
compiler output live. Do not run `cargo check`/`cargo build` inside
`pureshot/src-tauri` yourself unless explicitly asked — it races with their
file watcher and produces confusing interleaved/duplicate error output (this
has happened and the user asked to stop).

If you need to verify a `windows` crate binding or a self-contained native
file compiles before handing it back, check it in an isolated throwaway crate
that depends only on `windows` with the same feature list as
`pureshot/src-tauri/Cargo.toml`:

```sh
mkdir -p /tmp/opencode/npin
cat > /tmp/opencode/npin/Cargo.toml <<'EOF'
[package]
name = "npin"
version = "0.0.0"
edition = "2021"

[lib]
path = "lib.rs"

[dependencies]
windows = { version = "0.62", features = [
    "Win32_Foundation",
    "Win32_Graphics_Gdi",
    "Win32_System_LibraryLoader",
    "Win32_UI_Input_KeyboardAndMouse",
    "Win32_UI_Input_Pointer",
    "Win32_UI_WindowsAndMessaging",
] }
EOF
cp pureshot/src-tauri/src/native_pin.rs /tmp/opencode/npin/lib.rs
cd /tmp/opencode/npin && cargo check --target x86_64-pc-windows-msvc --offline
```

This requires the `x86_64-pc-windows-msvc` target (`rustup target add
x86_64-pc-windows-msvc`) even though the host is Linux — `cargo check` only
needs the target's `rust-std`, not an actual MSVC toolchain, since it stops
after type/borrow checking and never links.

This only works for files that don't depend on the rest of the Tauri app
(like `native_pin.rs`, which only uses `std` and `windows`). For changes to
`lib.rs` that touch Tauri APIs, read the code carefully instead and let the
user's `tauri dev` be the compiler check.
