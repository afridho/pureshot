# Capture settings verification

These checks require the native app, not only the browser preview. Restart the app after updating Rust. On GNOME, reinstall/update the companion extension and log out/in before testing its new D-Bus methods.

## Capture and preview

- Toggle **Hide desktop icons while capturing** and capture an area and fullscreen. Icons should be absent from the image and return immediately afterward. Cancel a selection and verify restoration. Icons hidden before capture must remain hidden.
- On GNOME, icon suppression requires supported DESKTOP window actors; unsupported desktop-icons extensions return an error. Portal window capture rejects icon suppression rather than silently ignoring it.
- Disable **Show after capture**. Area, fullscreen, shortcut, and tray captures should still enter History and auto-copy when enabled, without opening a preview. Restoring an item from History should still open a preview.
- With **Show on hover** enabled, preview actions appear on pointer entry. Disabled, actions remain visible. Hovering should still pause auto-close in both modes. Test a newly created preview for native Windows changes.
- Toggle **Show crosshair**. The selector should switch between pointer-aligned guides/cross cursor and no guides/ordinary cursor; the selection outline remains visible.
- Toggle **Show magnifier**. A zoomed pixel view should follow the pointer only when enabled. Test monitor edges and mixed-DPI monitors.
- With animated content visible, compare **Freeze screen** enabled and disabled. Enabled uses the pre-selection frame for both preview and output; disabled shows/captures live content. Windows live capture requires Windows 10 2004+ and compositor capture exclusion; otherwise it safely falls back to a frozen frame and logs a warning.
- Repeat selection cancellation using Escape and right-click. On GNOME, also test lock/unlock, extension disable, and monitor layout change during selection.

## Settings and restart

- Close/reopen settings and fully restart the app; saved capture preferences should remain selected.
- Hardware acceleration changes are persisted natively and apply only after full process restart. On Windows disabled mode adds `--disable-gpu` to WebView2 browser arguments. On Linux it disables WebKit accelerated compositing, not necessarily every GPU feature.
- **Include window shadow** must be disabled and labeled “Coming soon”. Transparent background, always-on-top, and restore-last-overlay toggles must not appear. Pins remain always-on-top by default.
- Drag the corner-radius slider continuously, release outside its bounds, and use keyboard arrows plus blur; verify the final value persists.
- Check settings labels, descriptions, disabled controls, and preview action visibility in light and dark desktop themes.

## Automated checks

- `./node_modules/.bin/tsc --noEmit`
- `node --check gnome-extension/extension.js`
- `git diff --check`
- Windows selector compile-check can run in an isolated copied crate with `cargo check --tests --target x86_64-pc-windows-msvc --offline`, without building the running Tauri application.

Dedicated Windows `capture_screen("window")` still follows the existing fullscreen implementation; this settings change does not implement a new dedicated window capture engine.
