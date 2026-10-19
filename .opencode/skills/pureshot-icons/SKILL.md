---
name: PureShot Icons
description: Use when adding, replacing, styling, or rasterizing icons in PureShot's React UI or native Win32 layered windows. Keeps native icons consistent with Lucide React and prevents jagged alpha/compositing artifacts.
---

# PureShot Icons

Use this skill whenever work involves icons in PureShot, especially icons rendered by native Win32 layered windows.

## Sources of truth

PureShot's frontend uses `lucide-react` through:

- `src/icons.tsx`
- `node_modules/lucide-react/dist/esm/icons/`

Before introducing an icon:

1. Check whether it is already exported by `src/icons.tsx`.
2. For React/WebView UI, import it from `src/icons.tsx`, not directly from a different icon library.
3. For native Win32 UI, use the same Lucide icon definition as the frontend so both implementations match.
4. Do not hand-invent an approximation when an official Lucide icon exists.

Current native Quick Access Overlay assets:

- `src-tauri/src/assets/overlay-icons/pin.png`
- `src-tauri/src/assets/overlay-icons/pencil.png`
- `src-tauri/src/assets/overlay-icons/x.png`
- `src-tauri/src/assets/overlay-icons/download.png`

They are high-resolution RGBA rasterizations of the official Lucide SVG icons used by `src/App.tsx`.

## Frontend workflow

For React UI:

1. Add the Lucide export to `src/icons.tsx` if it is not already exported.
2. Import the icon from `./icons`.
3. Match neighboring icon size, stroke width, color, and accessibility behavior.
4. Prefer `currentColor` so the surrounding button controls hover and pressed colors.
5. Run `./node_modules/.bin/tsc --noEmit` after non-trivial TypeScript changes.

Example:

```tsx
import { Download } from "./icons";

<button aria-label="Save">
  <Download size={15} />
</button>
```

## Native Win32 workflow

Native layered windows cannot render React SVG directly. Use this pipeline:

1. Read the exact Lucide definition from:
   `node_modules/lucide-react/dist/esm/icons/<icon>.js`.
2. Rasterize that official SVG at high resolution, preferably 192x192 RGBA.
3. Store the result under an appropriate directory in:
   `src-tauri/src/assets/`.
4. Embed the asset with `include_bytes!` so runtime rendering never depends on the network or filesystem layout.
5. Decode it once using `OnceLock`, not on every repaint.
6. Retain the alpha channel and tint/composite it in the native renderer.
7. Downsample with area sampling or multi-sample filtering. A single nearest-neighbor or single bilinear sample is insufficient when reducing 192px artwork to approximately 14px.

Reference implementation:

- `src-tauri/src/native_overlay_window.rs`
- `IconBitmap`
- `decode_icon`
- `icon_bitmap`
- `icon_coverage`

## Anti-aliasing rules

Small native icons and control backgrounds need explicit anti-aliasing.

- Use a high-resolution source image.
- For extreme downscaling, sample multiple subpixel locations per destination pixel (currently 4x4 in the Quick Access Overlay).
- For procedural circles and rounded rectangles, calculate coverage with subpixel supersampling rather than a hard inside/outside test.
- Treat analytic SDF AA width as physical pixels once geometry is already DPI-scaled. Do not multiply the AA width by DPI a second time.
- Preserve partial alpha at edges. Never threshold edge pixels into fully opaque pixels.
- Do not recreate detailed Lucide paths with crude line segments or approximate SDF shapes; this changes the icon and usually produces jagged diagonals.

## Premultiplied alpha rules

PureShot's native layered windows use `UpdateLayeredWindow` with `AC_SRC_ALPHA`; surfaces must contain premultiplied BGRA.

For a foreground color `(fr, fg, fb)` with alpha `fa` over background `(br, bg, bb)`:

```text
r = fr * fa + br * (1 - fa)
g = fg * fa + bg * (1 - fa)
b = fb * fa + bb * (1 - fa)
a = fa + ba * (1 - fa)
```

When writing the final DIB pixel:

```text
B = b * a * 255
G = g * a * 255
R = r * a * 255
A = a * 255
```

Do not add an unpremultiplied foreground color directly, such as:

```text
r = fr + br * (1 - fa)
```

That creates bright halos and jagged-looking edges.

## Hover and pressed states

Native controls should keep state separate:

- `hovered`: pointer is anywhere over the card.
- `hovered_zone`: exact control under the pointer.
- `pressed`: control captured on mouse down.

On `WM_MOUSEMOVE`:

1. Hit-test in card-local coordinates.
2. Update `hovered_zone`.
3. Repaint only when the zone changes or another visual state changes.

On `WM_MOUSELEAVE`:

1. Clear `hovered`.
2. Clear `hovered_zone`.
3. Clear `pressed`.
4. Repaint.

Hover may increase control brightness/opacity and icon contrast. Pressed state should remain visually distinct from hover.

## DPI and coordinate rules

- UI constants are defined in logical 96-DPI pixels.
- Button dimensions, margins, and icon size are multiplied by monitor scale once.
- Raster surfaces and Win32 window dimensions use physical pixels.
- Keep card-local geometry separate from the full layered-window canvas when an outer drop shadow adds transparent margins.
- Subtract the shadow margin before hit-testing pointer coordinates.
- Add the shadow margin when drawing GDI labels into the full DIB.

## Shadows and icon visibility

For a real outer shadow, reserve transparent pixels around the card. A shadow cannot extend beyond the layered-window bounds.

- Keep the visible card size unchanged.
- Expand only the native window/canvas.
- Position the expanded window so the card remains at its original screen coordinates.
- Render the shadow before the card and controls.
- Do not use a visible border unless explicitly requested.

## Adding or replacing a native icon

Checklist:

1. Confirm the intended Lucide icon in `src/icons.tsx` or the local Lucide package.
2. Create a transparent high-resolution RGBA asset.
3. Put it under `src-tauri/src/assets/<feature>/`.
4. Embed it with `include_bytes!`.
5. Add a `OnceLock<IconBitmap>` cache.
6. Map the relevant action/hit zone to the bitmap.
7. Use multi-sample downscaling.
8. Keep icon color separate from alpha so hover tinting can happen at runtime.
9. Check normal, hover, pressed, 100%, 125%, 150%, and 200% DPI rendering.
10. Run `rustfmt --edition 2021` and `git diff --check`.

## Verification constraints

Follow `AGENTS.md`:

- Do not run the normal `cargo check` or `cargo build` against the active Tauri target directory while the user's dev server may be running.
- Use an isolated target directory only when validation is necessary.
- A Windows-target check from WSL may stop in `tauri-winres` if `llvm-rc` is unavailable; report that as an environment limitation, not a source-code failure.
- Native Rust changes require a manual restart:

```powershell
Get-Process pureshot,rustc -ErrorAction SilentlyContinue | Stop-Process -Force
npm run tauri dev
```
