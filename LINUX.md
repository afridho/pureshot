# Linux support

PureShot uses **XDG Desktop Portal Screenshot** for still-image capture on Linux. No recording or cloud service is involved. The existing Win32 backend remains in place on Windows.

## Runtime requirements

Install `xdg-desktop-portal` and the appropriate screenshot-capable backend for your desktop (for example `xdg-desktop-portal-gnome` or `xdg-desktop-portal-kde`). Run PureShot inside a graphical desktop session with a session D-Bus. The desktop may ask for screenshot permission.

- **Fullscreen** requests a non-interactive screenshot; the desktop may still require consent.
- **Area on GNOME Wayland** requires the PureShot companion **GNOME Shell 50** extension in `gnome-extension/` and an explicitly authorized PureShot executable. The extension's `SelectArea` method displays a lightweight modal crosshair overlay over the **live desktop**, not a frozen GTK frame or GNOME screenshot toolbar. Left-drag and release captures; Escape or right-click cancels without saving. The overlay is removed and the compositor repaints before Shell captures the selected rectangle, cursor excluded. The configured corner radius is applied to the returned physical pixels. If the extension is absent, outdated or denies access, PureShot reports an error without silently opening GNOME's picker. No global Shell unsafe mode is enabled. Other desktop area backends are not yet implemented.
- **Area display mapping:** Shell owns stage coordinates and physical screenshot scaling, removing the old GTK single-monitor/integer-scale restriction. Monitor changes abort an active selection. Multi-monitor and fractional-scale results still require real-session testing; Shell determines the capture scale for areas spanning differently scaled monitors.
- **Window on GNOME 50** uses the authorized companion extension: hover highlights the topmost application window, with a Lucide camera cursor; left-click captures that window (including its frame), Escape/right-click cancels. The selected window is focused before Shell captures its surface, excluding the cursor and overlay. Closed/unfocusable windows fail safely. Update the app and extension together; there is no portal picker fallback. Other desktops do not yet have this window selector. Area crosshair/magnifier/freeze preferences do not apply to window selection.
- Quick preview, history, and annotation editing use WebViews. Quick Access previews use a fixed **16:10** canvas (336×210 logical pixels), with screenshots filling the canvas without distortion (`object-fit: cover`; excess edges are cropped). Screenshot pins and annotation pins use native GTK windows and preserve the screenshot's own aspect ratio, not 16:10.
- Wayland compositors may ignore window positioning and always-on-top hints. The GNOME companion marks authorized `PureShot Pin` windows always-on-top; load its updated code with logout/login. Other Wayland desktops may require compositor-specific rules. Native Windows crosshair/magnifier behavior is not implemented on Linux.
- Global shortcuts are desktop-dependent, particularly on Wayland. Failure to register one is logged and no longer prevents startup. **Settings → Shortcuts** supports assigning or clearing each binding; **Not assigned** stays disabled after restart. **Capture text (OCR)** defaults to unassigned and requires models installed through **Settings → Screenshots → OCR**. It selects an area and copies recognized text without opening Quick Access. Tray capture actions remain available even when their shortcuts are cleared.
- Some desktops do not show tray icons without an extension. Normal startup still shows settings even if tray creation fails. Avoid tray-only autostart on desktops without a tray.

Exports default to the desktop Pictures directory (falling back to the home directory), in `PureShot/`. Cached captures are under `$XDG_CACHE_HOME/PureShot/captures` or `~/.cache/PureShot/captures`.

## Development

Install the [Tauri Linux prerequisites](https://v2.tauri.app/start/prerequisites/) for your distribution, then run:

```sh
npm install
npm run tauri dev
```

The Windows capture crate is included at `vendor/capture-overlay`, so a separate sibling checkout is not needed. Cargo can resolve the Windows-only dependency on Linux without external source folders.

## GNOME dock identity

`app.enableGTKAppId` registers windows as `com.pureshot.app`, matching the desktop entry and icon name. GTK registration also means only one application instance can own that session-bus ID; stop an installed instance before starting `tauri dev`.

For a local executable/dev build, install the per-user launcher and icons (no sudo):

```sh
python3 scripts/install_linux_desktop.py --executable "$PWD/src-tauri/target/debug/pureshot"
```

Use the actual executable path; omit the option for `~/.local/bin/pureshot`. Restart the rebuilt app to apply its GTK identity. If GNOME still has an old pinned placeholder, unpin it and pin the restarted window. The installer preserves existing launchers and installs a `pureshot` icon alias for legacy windows. Packaged distributions should likewise install `com.pureshot.app.desktop` and the matching icon.

## Automated preview rendering check

With PyGObject, GTK3, and WebKit2 4.1 available, run `python3 scripts/test_linux_preview.py` inside the graphical session. It renders the actual preview CSS and verifies landscape, portrait, and square captures retain a 336×210 (16:10) canvas with `object-fit: cover`; pinned previews retain `contain`.

## Manual smoke tests

Test native area/window selection on GNOME Wayland with the companion extension installed and this executable authorized. Test portal fullscreen separately on desktops with a screenshot-capable portal:

1. Reinstall the updated extension and **log out/in** to load it (do not restart Shell under Wayland). Launch the app and capture fullscreen. Capture an area over a moving clock/video: the desktop must remain live. Test reverse drags, display edges, multiple displays, HiDPI/fractional scaling and exact physical dimensions in an image editor. Output must contain neither overlay nor cursor. For window capture, verify the camera cursor, hover highlight, overlapping windows/dialogs, click selection, Escape/right-click, a window closing during selection, and no capture when clicking the desktop. Check window output at fractional scale and with hide-desktop-icons enabled.
2. Cancel with Escape/right-click: no new history item. Settings return only if visible before area capture. Invoke the area shortcut with settings hidden: it should not flash settings or incur the visible-window hide delay. Test lock/disable during selection and allowlist revocation before releasing; ensure the modal disappears and future captures work.
3. Disable the extension or remove the executable from its allowlist: area/window capture should return settings and an actionable error, without opening GNOME's picker. For fullscreen capture, deny portal permission and verify error recovery.
4. Copy a screenshot and paste into an image editor; save each supported export format.
5. Open history from the tray and restore a capture.
6. Pin a capture, resize/drag/close it, capture again, and verify the old pin retains its own image.
7. Annotate, copy/save the result, and pin the annotation.
8. Check startup with conflicting shortcuts and a desktop without tray support. Clear each binding (including all bindings), restart, verify it remains Not assigned and the former key no longer captures, then reassign. Assign OCR, verify model-missing feedback before setup and area-to-text clipboard output after setup. Cancel recording with Escape/Cancel, and test conflicting assignments leave the previous binding working.
9. In Settings, verify the PureShot logo appears in the sidebar and About page in both dev and packaged builds. The frameless window should have only one SVG X control; clicking it minimizes (does not exit) and the window can be restored from the desktop task switcher/dock. Check dragging the top strip and light/dark themes.
10. After capture, hover Quick Access: only Copy/Save and the icon actions should appear, with no Extract Text action. OCR remains available from History and Settings → Screenshots.

### GNOME Shell crash / unexpected logout

On Wayland, a GNOME Shell compositor abort terminates the graphical session; an app-only crash normally does not. The observed Shell 50.1 journal contains a Clutter child-destruction assertion with frames in `dash.js` during dash-item removal. This does not establish the PureShot selector as the direct cause. Check GNOME 50 compatibility of dock/dash extensions and reproduce in a clean test session before capture stress testing; save work first. PureShot must not monkey-patch the Shell dash or suppress native assertions as a workaround.

Native Rust changes require restarting `npm run tauri dev`. Extension changes require reinstalling with `gnome-extension/install.py` and manually logging out/in on Wayland (save work first); restarting only the app does not reload Shell extension code.
