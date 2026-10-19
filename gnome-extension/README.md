# PureShot GNOME 50 companion

## Native window corners

The companion rounds trusted PureShot Settings and editor windows directly in
the compositor (20px, anti-aliased). Maximized/fullscreen windows remain square.
This is independent of Rounded Window Corners Reborn; it does not copy or import
that extension. Preview stacks, pins, and history are deliberately excluded.

Linux Wayland preview windows use the surface app ID `com.pureshot.preview`.
For Rounded Window Corners Reborn, blacklist this exact ID (no wildcard).
Alternatively keep PureShot's general classes blacklisted: the companion still
rounds Settings/editors itself. Avoid enabling both rounding implementations on
the same Settings window. Existing preview windows need an app restart for the
new identity. Companion code updates require reinstall and logout/login.

Local, privileged **GNOME Shell 50-only** extension. Area capture shows a small
modal overlay directly over the **live desktop**, not a frozen screenshot or
GNOME's built-in screenshot toolbar. Drag with the left button and release to
capture; Escape or right-click cancels. Only after removing the overlay and a
compositor repaint does Shell capture the selected area, without the cursor.
No unsafe mode is enabled. Installing/enabling this extension grants capture
privileges to the explicitly listed executables; it is not a generic public
screenshot service.

Window capture also uses this companion, via
`SelectWindowWithOptions(s optionsJson) -> (s path)`. Hover highlights the topmost
visible normal window or dialog under a Lucide Camera cursor; left-click selects,
Escape/right-click cancels with an empty path. Desktop/dock/minimized windows are
not targets. After modal teardown, the selected window is activated and checked
against Mutter's focused window before `screenshot_window(true, false, stream,
callback)` captures its surface with its frame and without the cursor. A closed
or unfocusable target fails rather than capturing a different window. Authorization,
lock/disable/disconnect cancellation and private output use the same service as
area capture. Hide-desktop-icons is supported; area-only crosshair, magnifier and
freeze settings are ignored. Reinstall and log out/in after updating both app and
extension; no portal fallback is opened when the companion is unavailable.

## Install / authorize

Run as the desktop user, **not with sudo**:

```sh
python3 gnome-extension/install.py \
  --allow-executable /home/afridho/projects/pureshot/src-tauri/target/debug/pureshot
```

Paths must already exist and be executable native ELF binaries. The installer
resolves symlinks and records canonical absolute paths. Repeat
`--allow-executable` to trust multiple builds. Do not authorize interpreters,
terminal programs, shells, generic D-Bus clients, or arbitrary downloaded binaries.
A different build path must be explicitly authorized; rebuilding at the same
path does not require reauthorization (trust is path-based, not a binary hash).

The installer copies only `metadata.json` and `extension.js` to
`~/.local/share/gnome-shell/extensions/pureshot@afridho`, backs up any previous
installation under `~/.local/share/gnome-shell/pureshot-extension-backups/`, and
attempts `gnome-extensions enable pureshot@afridho`. If Shell has not discovered
it yet, **log out and back in**, then enable it:

```sh
gnome-extensions enable pureshot@afridho
gnome-extensions info pureshot@afridho
```

Wayland also requires logout/login to reload installed code updates. The
installer never restarts Shell, logs out, or changes unsafe mode. `--no-enable`
only installs. XDG config/data/cache directory overrides are respected.

## Capture shortcuts

The extension registers area, window, fullscreen and OCR shortcut actions with
GNOME's compositor. PureShot Settings keeps these bindings synchronized with its
saved shortcuts. OCR starts **unassigned**; clearing any shortcut writes an empty
GSettings array (`[]`), disabling that binding without disabling tray/app controls.
`capture-ocr-shortcut` emits the `ocr` action and the app directly starts text
capture; it is not a screenshot capture mode. Reinstall and log out/in after
updating to add the OCR schema key and mapping.

## Native window integration

For allowlisted PureShot processes only, the extension positions windows titled
`PureShot Preview` at the bottom-right of the monitor work area (24 px margin)
and `PureShot History` flush at the physical monitor's bottom-left (no dock/work-area gap).
Previews, history and `PureShot Pin` windows are marked always-on-top using Mutter.
Preview/history positioning is recalculated when the frame size changes, keeping
the controls inside the work area after GTK/WebKit configuration. This is needed
because Wayland clients cannot force global positioning or above-window stacking.
Pins remain movable and are not repositioned by this hook. Installed code updates
require logout/login, including this window integration.

## Contract

- Session bus name **and** interface: `org.pureshot.Capture`
- Object: `/org/pureshot/Capture`
- Methods: `Capture() -> (s path)` for the entire desktop;
  `SelectArea(u cornerRadius) -> (s path)` for live drag selection and capture
  (used by Linux area capture). `cornerRadius` is an unsigned 32-bit radius in
  **output image pixels**, including zero for square corners. App and extension
  must both be updated for this input signature; older no-argument clients are
  intentionally incompatible.
- Return: **absolute local filesystem path**, not a URI, to a newly created PNG;
  `SelectArea` returns an empty string on user cancellation (Rust maps this to `None`).
- Errors: `org.pureshot.Capture.AccessDenied`, `.Busy`, or `.Failed`
- Both methods share authorization, reauthorization, private output and busy handling.
- Cursor excluded, no flash or clipboard changes. Shell chooses physical output
  resolution from the logical area and monitor scale; Rust preserves those pixels
  and applies the configured source-pixel corner radius. The live preview dims
  outside a rounded hole and draws a white border on that same path. It queries
  `Stage.get_capture_final_size(Mtk.Rectangle)` just like Shell's capture code,
  divides the clamped pixel radius by the returned scale, and recomputes while
  dragging across monitors. Fractional framebuffer rounding can differ by less
  than one pixel on tiny selections. No GTK selector is opened.

The trusted native process must make the bus call itself; spawning `gdbus` or
`busctl` will be denied because their executables are not PureShot. Introspection
is public, capture is not. Only one capture/authorization request runs at once.

## Security and lifecycle

On every request the extension asks the bus daemon for the unique sender's Unix
PID and checks `/proc/PID/exe` using `Gio.File` symlink metadata. That exact path
must occur in `~/.config/pureshot-extension/allowed-executables.json`, a JSON
array of canonical absolute paths. No caller-supplied identity or output path is
accepted. Missing/malformed allowlists, deleted executables, unreadable procfs,
unsafe ownership/permissions, and untrusted senders fail closed. Sender PID,
executable and allowlist are checked again before returning the result.

The config directory and allowlist must belong to the current user and have no
group/world permissions (installer creates 0700/0600). Revoke a path by removing
it from the array; the file is reread without restarting Shell. Disable the
extension to revoke the whole bridge:

```sh
gnome-extensions disable pureshot@afridho
```

Images are exclusively created with `Gio.FileCreateFlags.PRIVATE` (0600), UUID
names, in `~/.cache/pureshot-extension-captures` (0700). Existing unsafe or
symlinked cache directories are rejected; output files are never replaced.
Failed/disabled captures are removed. Successful captures are left for the
trusted client to read and **delete after ingestion**; old files can be manually
removed from that private cache. No automatic deletion of a capture another
request might still be reading occurs.

Capture is refused while locked, at the greeter, after disabling, and when the
desktop lockdown `disable-save-to-disk` policy is set. Lock/policy changes,
monitor changes, caller disconnection and extension disable abort an active
selection and immediately release its modal grab. Paint waits remove their signal,
idle and timeout sources on completion/abort. Extension defaults are the normal
user session only (no lock-screen session mode declared).

This is an explicit local user trust boundary, **not a sandbox against malicious
code already running as that same Unix user**. Such code can modify the
allowlist, replace a trusted path, or read same-user files; ptrace/injection,
exec/PID races and a compromised trusted binary cannot be fully prevented by
path-based authorization. Do not put trusted builds in directories writable by
other users. D-Bus name ownership alone is not proof a service is genuine if
malicious code already controls the same session bus.

## API provenance and validation

Implementation follows the actual GNOME Shell **50.0** sources:

- [shell-screenshot.h](https://github.com/GNOME/gnome-shell/blob/50.0/src/shell-screenshot.h):
  `screenshot(include_cursor, GOutputStream, callback)` and
  `screenshot_finish(result)` returning success and area.
- [screenshot.js](https://github.com/GNOME/gnome-shell/blob/50.0/js/ui/screenshot.js):
  `ScreenshotAsync` writes to a stream and closes it after capture.
- Verified against actual upstream `gnome-50` sources during the live-selector change:
  [shell-screenshot.c](https://github.com/GNOME/gnome-shell/blob/gnome-50/src/shell-screenshot.c)
  confirms `screenshot_area(x, y, width, height, stream, callback)` uses
  `NO_CURSORS` and `clutter_stage_get_capture_final_size`, not a logical-size crop.
  [main.js](https://github.com/GNOME/gnome-shell/blob/gnome-50/js/ui/main.js)
  confirms `pushModal(actor)` / `popModal(grab)`; Mutter's
  [clutter-stage.c](https://github.com/GNOME/mutter/blob/gnome-50/clutter/clutter/clutter-stage.c)
  confirms `after-paint` fires after painting. Capture resumes in a subsequent idle
  to avoid recursively painting inside that signal.
- Rounded preview APIs verified against **50.0** upstream sources:
  [barLevel.js](https://github.com/GNOME/gnome-shell/blob/50.0/js/ui/barLevel.js)
  uses `St.DrawingArea.get_context()`, `get_surface_size()`, Cairo arcs,
  `queue_repaint()` and `cr.$dispose()`;
  [st-drawing-area.c](https://github.com/GNOME/gnome-shell/blob/50.0/src/st/st-drawing-area.c)
  applies Cairo device scale, so paths stay in logical coordinates.
  [clutter-stage.c](https://github.com/GNOME/mutter/blob/50.0/clutter/clutter/clutter-stage.c)
  confirms `get_capture_final_size` returns success, rounded pixel dimensions
  and the maximum intersecting stage-view scale. The installed Mutter 18 typelib
  also exposes `get_capture_final_size(rect)` and `Mtk.Rectangle` to GJS.
- GNOME 50's exported `SelectArea` in `screenshot.js` is live, but its PanGesture
  and 200ms fade do not provide our right-click cancellation/public abort contract.
  PureShot therefore uses its own lightweight St actors, never `ScreenshotUI`.
- [ES module extension guide](https://gjs.guide/extensions/overview/imports-and-modules.html):
  default exported `Extension` subclass with `enable()`/`disable()`.

The implementation deliberately uses the callback form, which GJS preserves
when Shell has already promisified this method. Standalone `gjs` cannot execute
Shell's resource imports or take a screenshot outside the compositor. Static
syntax and standalone Gio security-helper checks do not substitute for enabling
inside a fresh GNOME 50 session and testing both an authorized PureShot capture
and an unauthorized D-Bus capture. Verify output permissions with `stat` and
ensure unauthorized calls return `AccessDenied`. Do not temporarily authorize a
generic bus client to test successful capture.

Headless regression tests: `node --test gnome-extension/test-*.cjs`.
These exercise drag geometry, cancellation, modal release and repaint-wait cleanup
with mocks, not GI bindings or actual compositor behavior. After reinstalling and
logging out/in, test a moving clock/video behind the selector, reverse drags,
Escape/right-click, tiny clicks, multi-monitor/fractional scaling, lock/disable
while selecting, allowlist revocation during selection, and repeated requests.
Confirm output has no overlay/cursor, retains physical pixel dimensions and rounded
corners, and a hidden-app area shortcut does not flash the settings window.
