import Clutter from 'gi://Clutter';
import St from 'gi://St';
import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import Meta from 'gi://Meta';
import Shell from 'gi://Shell';
import Mtk from 'gi://Mtk';
import Cairo from 'cairo';

import {Extension, InjectionManager} from 'resource:///org/gnome/shell/extensions/extension.js';
import * as AppFavorites from 'resource:///org/gnome/shell/ui/appFavorites.js';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';
import {installWindowCorners} from './window-corners.js';

const BUS_NAME = 'org.pureshot.Capture';
const OBJECT_PATH = '/org/pureshot/Capture';
const IFACE = `<node>
  <interface name="org.pureshot.Capture">
    <method name="Capture"><arg name="path" type="s" direction="out"/></method>
    <method name="CaptureWithOptions"><arg name="optionsJson" type="s" direction="in"/><arg name="path" type="s" direction="out"/></method>
    <method name="SelectArea"><arg name="cornerRadius" type="u" direction="in"/><arg name="path" type="s" direction="out"/></method>
    <method name="SelectAreaWithOptions"><arg name="cornerRadius" type="u" direction="in"/><arg name="optionsJson" type="s" direction="in"/><arg name="path" type="s" direction="out"/></method>
    <method name="SelectWindowWithOptions"><arg name="optionsJson" type="s" direction="in"/><arg name="path" type="s" direction="out"/></method>
    <signal name="ShortcutActivated"><arg name="mode" type="s"/></signal>
  </interface>
</node>`;
// Must match schemas/org.gnome.shell.extensions.pureshot.gschema.xml.
const SHORTCUT_KEYS = {
    'capture-area-shortcut': 'area',
    'capture-window-shortcut': 'window',
    'capture-fullscreen-shortcut': 'fullscreen',
    'capture-ocr-shortcut': 'ocr',
};
const NOFOLLOW = Gio.FileQueryInfoFlags.NOFOLLOW_SYMLINKS;
const ATTRS = 'standard::type,standard::is-symlink,standard::size,unix::uid,unix::mode';

// Same non-destructive list filtering as Dock App Visibility. This deliberately
// hides the whole application, including Settings, not just capture previews.
function isPureShotApp(app) {
    return ['com.pureshot.app.desktop', 'pureshot.desktop', 'PureShot.desktop'].includes(app.get_id()) ||
        app.get_windows().some(window =>
            ['com.pureshot.app', 'pureshot'].includes((window.get_wm_class() ?? '').toLowerCase()));
}

function installDockVisibility() {
    const injections = new InjectionManager();
    injections.overrideMethod(Shell.AppSystem.prototype, 'get_running', original =>
        function (...args) {
            return original.apply(this, args).filter(app => !isPureShotApp(app));
        });
    injections.overrideMethod(AppFavorites.getAppFavorites(), 'getFavoriteMap', original =>
        function (...args) {
            return Object.fromEntries(Object.entries(original.apply(this, args))
                .filter(([_id, app]) => !isPureShotApp(app)));
        });
    return injections;
}

function privateInfo(file, type) {
    const info = file.query_info(ATTRS, NOFOLLOW, null);
    if (info.get_is_symlink() || info.get_file_type() !== type ||
        info.get_attribute_uint32('unix::uid') !== new Gio.Credentials().get_unix_user() ||
        (info.get_attribute_uint32('unix::mode') & 0o077) !== 0)
        throw new Error('Private PureShot file/directory has unsafe ownership or permissions');
    return info;
}

const SELECTION_DEFAULTS = Object.freeze({
    show_crosshair: true, show_magnifier: true,
    freeze_screen: false, hide_desktop_icons: false,
});

function selectionOptions(json) {
    if (typeof json !== 'string' || json.length > 1024)
        throw new Error('Invalid selection options');
    const parsed = JSON.parse(json);
    if (!parsed || Array.isArray(parsed) || typeof parsed !== 'object')
        throw new Error('Selection options must be an object');
    for (const [key, value] of Object.entries(parsed)) {
        if (!Object.hasOwn(SELECTION_DEFAULTS, key) || typeof value !== 'boolean')
            throw new Error(`Unsupported selection option: ${key}`);
    }
    return {...SELECTION_DEFAULTS, ...parsed};
}

// Round shared endpoints, not origin and extent independently: at fractional
// scale round(x*s) + round(w*s) can extend beyond the screenshot texture.
function frozenPixelArea([x, y, width, height], scale, textureWidth, textureHeight) {
    const clampX = value => Math.max(0, Math.min(textureWidth, Math.round(value * scale)));
    const clampY = value => Math.max(0, Math.min(textureHeight, Math.round(value * scale)));
    const left = clampX(x);
    const top = clampY(y);
    const right = clampX(x + width);
    const bottom = clampY(y + height);
    if (right <= left || bottom <= top)
        throw new Error('Frozen selection contains no screenshot pixels');
    return [left, top, right - left, bottom - top];
}

// GNOME 50's exported SelectArea is live, but its PanGesture accepts secondary
// drags and fades for 200ms; it has no public cancellation API. Keep our small
// overlay independent so right-click, lock and disable can release it immediately.
class LiveAreaSelection {
    constructor(cornerRadius = 0, options = SELECTION_DEFAULTS, frozen = null) {
        this.cornerRadius = cornerRadius;
        this.options = options;
        this.frozen = frozen;
        this.pointer = global.get_pointer().slice(0, 2);
        this.actor = new St.Widget({reactive: true, can_focus: true,
            x: 0, y: 0, width: global.stage.width, height: global.stage.height});
        this.signals = [];
        this._finishing = false;
        this._finishId = 0;
        this.drawing = new St.DrawingArea({width: global.stage.width, height: global.stage.height});
        if (frozen) {
            this.actor.add_child(new Clutter.Actor({content: frozen.content,
                width: global.stage.width, height: global.stage.height}));
        }
        this.actor.add_child(this.drawing);
        if (options.show_magnifier) {
            // A clipped, 4x texture viewport, not a decorative cursor bubble.
            this.lens = new St.Widget({width: 128, height: 128,
                clip_to_allocation: true, style: 'border: 2px solid white; background: black;'});
            this.zoom = new Clutter.Actor();
            // Mutter 50 ClutterActor.set_content_scaling_filters controls the
            // content texture's minification/magnification sampling.
            this.zoom.set_content_scaling_filters(
                Clutter.ScalingFilter.NEAREST, Clutter.ScalingFilter.NEAREST);
            this.lens.add_child(this.zoom);
            this.actor.add_child(this.lens);
            this.lensMark = new St.Label({text: '+', x: 59, y: 54,
                style: 'color: white; text-shadow: 0 1px 2px black;'});
            this.lens.add_child(this.lensMark);
        }
        this.drawing.connect('repaint', () => {
            if (this._finishing)
                return;
            let cr = null;
            try {
                cr = this.drawing.get_context();
                this._paint(cr);
            } catch (error) {
                this.finish(null, error);
            } finally {
                try {
                    cr?.$dispose();
                } catch (error) {
                    this.finish(null, error);
                }
            }
        });
        this.rect = [0, 0, 0, 0];
        this.radius = 0;
        this.start = null;
        this.grab = null;
    }

    select() {
        return new Promise((resolve, reject) => {
            this.resolve = resolve;
            this.reject = reject;
            try {
                Main.uiGroup.add_child(this.actor);
                this._updateMagnifier();
                if (this.lens && !this.frozen) {
                    this._zoomId = GLib.timeout_add(GLib.PRIORITY_DEFAULT, 50, () => {
                        try {
                            this._updateMagnifier();
                        } catch (error) {
                            this.finish(null, error);
                        }
                        return this._finishing ? GLib.SOURCE_REMOVE : GLib.SOURCE_CONTINUE;
                    });
                }
                this._draw([0, 0, 0, 0]);
                this.signals.push(this.actor.connect('captured-event', (_actor, event) => {
                    try {
                        const type = event.type();
                        // Mutter must see crossing events to maintain pointer focus.
                        if (type === Clutter.EventType.ENTER || type === Clutter.EventType.LEAVE)
                            return Clutter.EVENT_PROPAGATE;
                        if (this._finishing)
                            return Clutter.EVENT_PROPAGATE;
                        if (type === Clutter.EventType.KEY_PRESS &&
                            event.get_key_symbol() === Clutter.KEY_Escape) {
                            this.finish(null);
                        } else if (type === Clutter.EventType.BUTTON_PRESS) {
                            if (event.get_button() === Clutter.BUTTON_SECONDARY)
                                this.finish(null);
                            else if (event.get_button() === Clutter.BUTTON_PRIMARY)
                                this._press(event);
                            else
                                return Clutter.EVENT_PROPAGATE;
                        } else if (type === Clutter.EventType.MOTION) {
                            this.pointer = this._point(event);
                            this._updateMagnifier();
                            this._draw(this.start ? this._rect(event) : this.rect);
                        } else if (type === Clutter.EventType.BUTTON_RELEASE &&
                            event.get_button() === Clutter.BUTTON_PRIMARY && this.start) {
                            this._release(event);
                        } else {
                            return Clutter.EVENT_PROPAGATE;
                        }
                    } catch (error) {
                        this.finish(null, error);
                    }
                    return Clutter.EVENT_STOP;
                }));
                this.actor.show();
                this.grab = Main.pushModal(this.actor);
                // Draw a larger plus ourselves without changing the desktop's
                // cursor theme/size. Actor-owned hiding ends with modal teardown.
                this.actor.set_cursor_type(Clutter.CursorType.NONE);
            } catch (error) {
                this.finish(null, error);
            }
        });
    }

    _updateMagnifier() {
        if (!this.lens || this._finishing)
            return;
        const [x, y] = this.pointer;
        const size = 128;
        const zoom = 4;
        this.lens.set_position(
            Math.max(0, Math.min(global.stage.width - size, x + 24 > global.stage.width - size ? x - size - 24 : x + 24)),
            Math.max(0, Math.min(global.stage.height - size, y + 24 > global.stage.height - size ? y - size - 24 : y + 24)));
        if (this.frozen) {
            this.zoom.set_content(this.frozen.content);
            this.zoom.set_size(global.stage.width * zoom, global.stage.height * zoom);
            this.zoom.set_position(size / 2 - x * zoom, size / 2 - y * zoom);
        } else {
            // Synchronously paint only the small source region while our entire
            // overlay is transparent; restore before the compositor presents.
            // No recursive clone of the stage (which would include the lens).
            const side = size / zoom;
            const sx = Math.max(0, Math.min(global.stage.width - side, x - side / 2));
            const sy = Math.max(0, Math.min(global.stage.height - side, y - side / 2));
            const region = new Mtk.Rectangle({x: sx, y: sy, width: side, height: side});
            const [ok, , , scale] = global.stage.get_capture_final_size(region);
            if (!ok || !(scale > 0))
                throw new Error('Cannot determine live magnifier capture scale');
            this.actor.opacity = 0;
            let content;
            try {
                content = global.stage.paint_to_content(region,
                    scale, global.stage.get_color_state(), Clutter.PaintFlag.NO_CURSORS);
            } finally {
                this.actor.opacity = 255;
            }
            if (!content)
                throw new Error('Live magnifier is unsupported by this compositor');
            this.zoom.set_content(content);
            this.zoom.set_size(size, size);
            this.zoom.set_position(size / 2 - (x - sx) * zoom, size / 2 - (y - sy) * zoom);
        }
    }

    _point(event) {
        const [x, y] = event.get_coords();
        return [Math.max(0, Math.min(global.stage.width, Math.floor(x))),
            Math.max(0, Math.min(global.stage.height, Math.floor(y)))];
    }

    _press(event) {
        this.start = this._point(event);
    }

    _release(event) {
        const rect = this._rect(event);
        if (rect[2] > 0 && rect[3] > 0)
            this.finish(rect);
        else
            this.start = null;
    }

    _rect(event) {
        const [x, y] = this._point(event);
        return [Math.min(x, this.start[0]), Math.min(y, this.start[1]),
            Math.abs(x - this.start[0]), Math.abs(y - this.start[1])];
    }

    _draw([x, y, w, h]) {
        this.rect = [x, y, w, h];
        this.radius = 0;
        if (w > 0 && h > 0 && this.cornerRadius > 0) {
            // Same query as Shell.Screenshot's do_grab_screenshot: maximum
            // intersecting stage-view scale (not the UI/theme scale). Output
            // dimensions are round(logical size * scale), including mixed DPI.
            const [ok, pixelWidth, pixelHeight, scale] = global.stage.get_capture_final_size(
                new Mtk.Rectangle({x, y, width: w, height: h}));
            if (!ok || !(scale > 0))
                throw new Error('Cannot determine selection capture scale');
            const captureScale = this.frozen?.scale ?? scale;
            this.radius = Math.min(this.cornerRadius / captureScale, w / 2, h / 2);
            // Fractional framebuffer rounding can differ by <1 pixel; keep the
            // preview path valid for tiny selections as well.
            this.radius = Math.min(this.radius, w / 2, h / 2);
        }
        this.drawing.queue_repaint();
    }

    _path(cr) {
        const [x, y, w, h] = this.rect;
        const r = this.radius;
        if (r === 0) {
            cr.rectangle(x, y, w, h);
            return;
        }
        cr.newSubPath();
        cr.arc(x + w - r, y + r, r, -Math.PI / 2, 0);
        cr.arc(x + w - r, y + h - r, r, 0, Math.PI / 2);
        cr.arc(x + r, y + h - r, r, Math.PI / 2, Math.PI);
        cr.arc(x + r, y + r, r, Math.PI, Math.PI * 1.5);
        cr.closePath();
    }

    _paint(cr) {
        const [width, height] = this.drawing.get_surface_size();
        const selected = this.rect[2] > 0 && this.rect[3] > 0;
        if (selected) {
            // CleanShot-style selection: only the chosen region is shaded;
            // leave the surrounding desktop untouched, including rounded cutouts.
            this._path(cr);
            cr.setSourceRGBA(0, 0, 0, 0.15);
            cr.fill();
        }
        if (this.options.show_crosshair) {
            const [x, y] = this.pointer;
            cr.moveTo(x + 0.5, 0);
            cr.lineTo(x + 0.5, height);
            cr.moveTo(0, y + 0.5);
            cr.lineTo(width, y + 0.5);
            cr.setSourceRGBA(1, 1, 1, 0.7);
            cr.setLineWidth(1);
            cr.stroke();
        }
        if (selected) {
            this._path(cr);
            cr.setSourceRGBA(1, 1, 1, 1);
            cr.setLineWidth(1);
            cr.stroke();
        }
        // Compact crosshair spanning 20 logical pixels.
        // Keep its center at the exact selection coordinate, even without guides.
        const [x, y] = this.pointer;
        const halfSize = 10;
        cr.moveTo(x + 0.5 - halfSize, y + 0.5);
        cr.lineTo(x + 0.5 + halfSize, y + 0.5);
        cr.moveTo(x + 0.5, y + 0.5 - halfSize);
        cr.lineTo(x + 0.5, y + 0.5 + halfSize);
        cr.setSourceRGBA(0, 0, 0, 1);
        cr.setLineWidth(4);
        cr.strokePreserve();
        cr.setSourceRGBA(1, 1, 1, 1);
        cr.setLineWidth(2);
        cr.stroke();
    }

    finish(rect = null, error = null) {
        if (!this.resolve || this._finishing)
            return;
        this._finishing = true;
        if (this._zoomId) {
            GLib.source_remove(this._zoomId);
            this._zoomId = 0;
        }
        // Never destroy actors/pop the modal from repaint or input dispatch.
        // Track the one-shot source and settle only after teardown has run.
        this._finishId = GLib.idle_add(GLib.PRIORITY_DEFAULT, () => {
            this._finishId = 0;
            const resolve = this.resolve;
            const reject = this.reject;
            this.resolve = this.reject = null;
            const cleanup = action => {
                try {
                    action();
                } catch (cleanupError) {
                    error ??= cleanupError;
                }
            };
            const signals = this.signals;
            this.signals = [];
            for (const id of signals)
                cleanup(() => this.actor.disconnect(id));
            const grab = this.grab;
            this.grab = null;
            if (grab)
                cleanup(() => Main.popModal(grab));
            cleanup(() => this.actor.destroy());
            if (error)
                reject(error);
            else
                resolve(rect);
            return GLib.SOURCE_REMOVE;
        });
    }
}

// Use Mutter's stacking order, not stage picking: the modal covers every window.
class LiveWindowSelection extends LiveAreaSelection {
    constructor() {
        super(0, {show_crosshair: false, show_magnifier: false});
        this.pressedWindow = null;
    }

    select() {
        const result = super.select();
        if (!this._finishing) {
            this._zoomId = GLib.timeout_add(GLib.PRIORITY_DEFAULT, 50, () => {
                try {
                    this._draw();
                } catch (error) {
                    this.finish(null, error);
                }
                return this._finishing ? GLib.SOURCE_REMOVE : GLib.SOURCE_CONTINUE;
            });
        }
        return result;
    }

    _draw() {
        const [x, y] = this.pointer;
        const actors = global.get_window_actors();
        const windows = global.display.sort_windows_by_stacking(actors.map(actor => actor.meta_window));
        this.window = [...windows].reverse().find(window => {
            if (![Meta.WindowType.NORMAL, Meta.WindowType.DIALOG, Meta.WindowType.MODAL_DIALOG]
                .includes(window.get_window_type()) || !window.showing_on_its_workspace() ||
                window.minimized || !window.get_compositor_private()?.is_visible())
                return false;
            const rect = window.get_frame_rect();
            return rect.width > 0 && rect.height > 0 && x >= rect.x && y >= rect.y &&
                x < rect.x + rect.width && y < rect.y + rect.height;
        }) ?? null;
        this.rect = [0, 0, 0, 0];
        this.radius = 0;
        if (this.window) {
            const {x: left, y: top, width, height} = this.window.get_frame_rect();
            this.rect = [left, top, width, height];
            // Match our compositor rounding in logical/stage pixels. This is
            // preview-only: do not mask or round the captured window surface.
            // Mutter has no universal corner-radius property for client windows.
            if (!this.window.is_fullscreen() && !this.window.maximizedHorizontally &&
                !this.window.maximizedVertically)
                this.radius = Math.min(20, width / 2, height / 2);
        }
        this.drawing.queue_repaint();
    }

    _press(event) {
        this.pointer = this._point(event);
        this._draw();
        this.start = this.pointer;
        this.pressedWindow = this.window;
    }

    _release(event) {
        this.pointer = this._point(event);
        this._draw();
        if (this.window && this.window === this.pressedWindow)
            this.finish(this.window);
        else
            this.start = null;
    }

    _paint(cr) {
        if (this.window) {
            this._path(cr);
            cr.setSourceRGBA(0, 0, 0, 0.15);
            cr.fillPreserve();
            cr.setSourceRGBA(1, 1, 1, 1);
            cr.setLineWidth(2);
            cr.stroke();
        }
        // Official Lucide Camera path, in its original 24x24 coordinate system.
        // Paint at the pointer hotspot, with a dark halo for either background.
        cr.save();
        cr.translate(...this.pointer);
        cr.scale(1.5, 1.5);
        cr.moveTo(14.5, 4);
        cr.lineTo(9.5, 4);
        cr.lineTo(7, 7);
        cr.lineTo(4, 7);
        cr.curveTo(2.8954, 7, 2, 7.8954, 2, 9);
        cr.lineTo(2, 18);
        cr.curveTo(2, 19.1046, 2.8954, 20, 4, 20);
        cr.lineTo(20, 20);
        cr.curveTo(21.1046, 20, 22, 19.1046, 22, 18);
        cr.lineTo(22, 9);
        cr.curveTo(22, 7.8954, 21.1046, 7, 20, 7);
        cr.lineTo(17, 7);
        cr.closePath();
        cr.newSubPath();
        cr.arc(12, 13, 3, 0, Math.PI * 2);
        cr.setLineJoin(Cairo.LineJoin.ROUND);
        cr.setLineCap(Cairo.LineCap.ROUND);
        cr.setSourceRGBA(0, 0, 0, 1);
        cr.setLineWidth(4);
        cr.strokePreserve();
        cr.setSourceRGBA(1, 1, 1, 1);
        cr.setLineWidth(2);
        cr.stroke();
        cr.restore();
    }
}

class CaptureService {
    constructor() {
        this.active = true;
        this.busy = false;
        this.lockdown = new Gio.Settings({schema_id: 'org.gnome.desktop.lockdown'});
    }

    _allowedExecutables() {
        // Reload on every request: revocation needs no Shell restart. Fail closed.
        const dir = Gio.File.new_for_path(GLib.build_filenamev([
            GLib.get_user_config_dir(), 'pureshot-extension',
        ]));
        privateInfo(dir, Gio.FileType.DIRECTORY);
        const file = dir.get_child('allowed-executables.json');
        if (privateInfo(file, Gio.FileType.REGULAR).get_size() > 65536)
            throw new Error('Allowlist is too large');
        const [ok, bytes] = file.load_contents(null);
        if (!ok)
            throw new Error('Cannot load allowlist');
        const paths = JSON.parse(new TextDecoder().decode(bytes));
        if (!Array.isArray(paths) || paths.length > 64 || paths.some(path =>
            typeof path !== 'string' || !GLib.path_is_absolute(path) ||
            GLib.canonicalize_filename(path, null) !== path))
            throw new Error('Allowlist must contain canonical absolute executable paths');
        return paths;
    }

    _senderPid(invocation) {
        const sender = invocation.get_sender();
        if (!sender?.startsWith(':'))
            throw new Error('Missing unique D-Bus sender');
        return new Promise((resolve, reject) => {
            invocation.get_connection().call(
                'org.freedesktop.DBus', '/org/freedesktop/DBus',
                'org.freedesktop.DBus', 'GetConnectionUnixProcessID',
                new GLib.Variant('(s)', [sender]), new GLib.VariantType('(u)'),
                Gio.DBusCallFlags.NONE, 3000, null, (connection, result) => {
                    try {
                        resolve(connection.call_finish(result).deep_unpack()[0]);
                    } catch (error) {
                        reject(error);
                    }
                });
        });
    }

    _executable(pid) {
        if (!Number.isInteger(pid) || pid <= 0)
            throw new Error('Invalid sender process');
        const info = Gio.File.new_for_path(`/proc/${pid}/exe`).query_info(
            'standard::is-symlink,standard::symlink-target', NOFOLLOW, null);
        const path = info.get_symlink_target();
        if (!info.get_is_symlink() || !path || !GLib.path_is_absolute(path) ||
            path.endsWith(' (deleted)'))
            throw new Error('Cannot identify sender executable');
        return path;
    }

    _checkSession() {
        if (!this.active || Main.sessionMode.isLocked || Main.sessionMode.isGreeter ||
            this.lockdown.get_boolean('disable-save-to-disk'))
            throw new Error('Capture is unavailable in this session');
    }

    _outputFile() {
        const path = GLib.build_filenamev([
            GLib.get_user_cache_dir(), 'pureshot-extension-captures',
        ]);
        if (GLib.mkdir_with_parents(path, 0o700) !== 0)
            throw new Error('Cannot create capture cache');
        const dir = Gio.File.new_for_path(path);
        privateInfo(dir, Gio.FileType.DIRECTORY);
        return dir.get_child(`${GLib.uuid_string_random()}.png`);
    }

    CaptureAsync(_params, invocation) {
        return this._capture(invocation, false);
    }

    SelectAreaAsync([cornerRadius], invocation) {
        return this._capture(invocation, true, cornerRadius);
    }

    CaptureWithOptionsAsync([json], invocation) {
        return this._capture(invocation, false, 0, json);
    }

    SelectAreaWithOptionsAsync([cornerRadius, json], invocation) {
        return this._capture(invocation, true, cornerRadius, json);
    }

    SelectWindowWithOptionsAsync([json], invocation) {
        return this._capture(invocation, 'window', 0, json);
    }

    _hideDesktopIcons() {
        // DING and legacy desktop-icons implementations expose DESKTOP window
        // actors. Hide only those surfaces, never the wallpaper/background group.
        // No GSettings writes or extension disable/enable cycles to leave behind.
        const saved = new Map();
        let mapId = 0;
        const restore = () => {
            if (mapId) {
                global.window_manager.disconnect(mapId);
                mapId = 0;
            }
            for (const [actor, opacity] of saved) {
                try {
                    actor.opacity = opacity;
                } catch {
                    // A desktop surface can disappear while selection is open.
                }
            }
            saved.clear();
        };
        const hide = actor => {
            if (actor.meta_window?.get_window_type() !== Meta.WindowType.DESKTOP)
                return;
            if (!saved.has(actor))
                saved.set(actor, actor.opacity);
            actor.opacity = 0;
        };
        // Shell-actor-only desktop extensions have no stable public actor API.
        // Reject them instead of falsely promising an icon-free screenshot.
        const supported = new Set(['ding@rastersoft.com', 'desktop-icons@csoriano']);
        const enabled = Main.extensionManager.getUuids().filter(uuid => {
            const extension = Main.extensionManager.lookup(uuid);
            return extension?.state === 1 && /desktop.*icons|^ding@/i.test(uuid);
        });
        if (enabled.some(uuid => !supported.has(uuid)))
            throw new Error('Hide desktop icons is unsupported for this desktop-icons extension');
        if (enabled.length && !global.get_window_actors().some(actor =>
            actor.meta_window?.get_window_type() === Meta.WindowType.DESKTOP))
            throw new Error('Desktop icons extension exposes no supported DESKTOP window actors');
        try {
            for (const actor of global.get_window_actors())
                hide(actor);
            mapId = global.window_manager.connect('map', (_wm, actor) => hide(actor));
        } catch (error) {
            restore();
            throw error;
        }
        return restore;
    }

    stop() {
        this.active = false;
        this._abort?.();
    }

    _afterPaint() {
        // after-paint is a real compositor repaint, not a fixed delay or an
        // idle that could run before redraw. Capture from the resumed promise,
        // outside the paint signal (screenshot_area itself paints the stage).
        return new Promise((resolve, reject) => {
            let paintId = 0;
            let timeoutId = 0;
            let idleId = 0;
            const finish = error => {
                if (paintId)
                    global.stage.disconnect(paintId);
                if (timeoutId)
                    GLib.source_remove(timeoutId);
                if (idleId)
                    GLib.source_remove(idleId);
                paintId = timeoutId = idleId = 0;
                this._cancelWait = null;
                if (error)
                    reject(error);
                else
                    resolve();
            };
            this._cancelWait = () => finish(new Error('Capture interrupted'));
            paintId = global.stage.connect('after-paint', () => {
                global.stage.disconnect(paintId);
                paintId = 0;
                // GJS can drain promise jobs at signal boundaries. Resume via
                // idle only after paint has unwound, avoiding recursive paint.
                idleId = GLib.idle_add(GLib.PRIORITY_DEFAULT, () => {
                    idleId = 0;
                    finish(null);
                    return GLib.SOURCE_REMOVE;
                });
            });
            timeoutId = GLib.timeout_add(GLib.PRIORITY_DEFAULT, 2000, () => {
                timeoutId = 0;
                finish(new Error('Compositor did not repaint'));
                return GLib.SOURCE_REMOVE;
            });
            global.stage.queue_redraw();
        });
    }

    async _capture(invocation, select, cornerRadius = 0, json = '{}') {
        // One in-flight request (including authorization) avoids unbounded captures.
        if (this.busy) {
            invocation.return_dbus_error(`${BUS_NAME}.Busy`, 'A capture is already in progress');
            return;
        }
        this.busy = true;
        let file = null;
        let stream = null;
        let success = false;
        let created = false;
        let authorized = false;
        let sessionId = 0;
        let policyId = 0;
        let monitorId = 0;
        let senderWatchId = 0;
        let interrupted = false;
        let restoreIcons = null;
        let frozen = null;
        this._abort = () => {
            restoreIcons?.();
            restoreIcons = null;
            interrupted = true;
            this._selection?.finish();
            this._cancelWait?.();
        };
        const check = () => {
            this._checkSession();
            if (interrupted)
                throw new Error('Capture interrupted');
        };
        try {
            const pid = await this._senderPid(invocation);
            const executable = this._executable(pid);
            if (!this._allowedExecutables().includes(executable))
                throw new Error('Caller executable is not authorized');
            authorized = true;
            check();
            const sessionChanged = () => {
                try {
                    check();
                } catch {
                    this._abort();
                }
            };
            sessionId = Main.sessionMode.connect('updated', sessionChanged);
            policyId = this.lockdown.connect('changed::disable-save-to-disk', sessionChanged);
            monitorId = Main.layoutManager.connect('monitors-changed', () => this._abort());
            senderWatchId = Gio.bus_watch_name_on_connection(invocation.get_connection(),
                invocation.get_sender(), Gio.BusNameWatcherFlags.NONE, null, () => this._abort?.());
            const options = selectionOptions(json);
            if (options.hide_desktop_icons) {
                restoreIcons = this._hideDesktopIcons();
                await this._afterPaint();
                check();
            }
            let area = null;
            let selectedWindow = null;
            if (select) {
                if (select !== 'window' && options.freeze_screen) {
                    const shooter = new Shell.Screenshot();
                    frozen = await new Promise((resolve, reject) => {
                        shooter.screenshot_stage_to_content((source, result) => {
                            try {
                                const [content, scale] = source.screenshot_stage_to_content_finish(result);
                                if (!content || !(scale > 0))
                                    throw new Error('Frozen capture is unsupported by this compositor');
                                resolve({content, scale});
                            } catch (error) {
                                reject(error);
                            }
                        });
                    });
                    check();
                }
                this._selection = select === 'window' ? new LiveWindowSelection() :
                    new LiveAreaSelection(cornerRadius, options, frozen);
                const selected = await this._selection.select();
                if (select === 'window')
                    selectedWindow = selected;
                else
                    area = selected;
                this._selection = null;
                check();
                if (area || selectedWindow)
                    await this._afterPaint();
                check();
            }
            // Reauthorize even cancellation; never release results to a caller
            // whose identity or trust changed while the modal was open.
            const selectedPid = await this._senderPid(invocation);
            if (selectedPid !== pid || this._executable(pid) !== executable ||
                !this._allowedExecutables().includes(executable))
                throw new Error('Caller authorization changed');
            check();
            if (select && !area && !selectedWindow) {
                invocation.return_value(new GLib.Variant('(s)', ['']));
                success = true;
                return;
            }
            if (selectedWindow) {
                if (!selectedWindow.get_compositor_private() ||
                    !selectedWindow.showing_on_its_workspace() || selectedWindow.minimized)
                    throw new Error('Selected window is no longer available');
                selectedWindow.activate(global.get_current_time());
                await this._afterPaint();
                check();
            }
            file = this._outputFile();
            // Exclusive creation, never replacement or a caller-provided path.
            stream = file.create(Gio.FileCreateFlags.PRIVATE, null);
            created = true;
            // Explicit callback form also works when Shell has promisified this API.
            const screenshot = new Shell.Screenshot();
            if (frozen) {
                await new Promise((resolve, reject) => {
                    const texture = frozen.content.get_texture();
                    const pixels = frozenPixelArea(area, frozen.scale,
                        texture.get_width(), texture.get_height());
                    Shell.Screenshot.composite_to_stream(texture,
                        ...pixels, frozen.scale, null, 0, 0, 1, stream, (_source, result) => {
                            try {
                                Shell.Screenshot.composite_to_stream_finish(result);
                                resolve();
                            } catch (error) {
                                reject(error);
                            }
                        });
                });
            } else await new Promise((resolve, reject) => {
                const callback = (source, result) => {
                    try {
                        const [ok] = selectedWindow ? source.screenshot_window_finish(result) :
                            area ? source.screenshot_area_finish(result) :
                            source.screenshot_finish(result);
                        if (!ok)
                            throw new Error('Screenshot failed');
                        resolve();
                    } catch (error) {
                        reject(error);
                    }
                };
                // GNOME 50 takes stage/logical coordinates, paints with
                // NO_CURSORS and chooses the physical capture size/scale.
                if (selectedWindow) {
                    if (!selectedWindow.get_compositor_private() ||
                        global.display.get_focus_window() !== selectedWindow)
                        throw new Error('Selected window could not be focused; capture cancelled');
                    screenshot.screenshot_window(true, false, stream, callback);
                } else if (area)
                    screenshot.screenshot_area(...area, stream, callback);
                else
                    screenshot.screenshot(false, stream, callback);
            });
            stream.close(null);
            stream = null;
            check();
            // Verify the sender is still the same executable before releasing data.
            const currentPid = await this._senderPid(invocation);
            if (currentPid !== pid || this._executable(pid) !== executable ||
                !this._allowedExecutables().includes(executable))
                throw new Error('Caller authorization changed');
            check();
            invocation.return_value(new GLib.Variant('(s)', [file.get_path()]));
            success = true;
        } catch (error) {
            invocation.return_dbus_error(
                `${BUS_NAME}.${authorized ? 'Failed' : 'AccessDenied'}`,
                authorized ? error.message : 'Caller is not an explicitly trusted PureShot executable');
        } finally {
            restoreIcons?.();
            restoreIcons = null;
            frozen = null;
            this._selection?.finish();
            this._selection = null;
            this._cancelWait?.();
            this._abort = null;
            if (sessionId)
                Main.sessionMode.disconnect(sessionId);
            if (policyId)
                this.lockdown.disconnect(policyId);
            if (monitorId)
                Main.layoutManager.disconnect(monitorId);
            if (senderWatchId)
                Gio.bus_unwatch_name(senderWatchId);
            try {
                stream?.close(null);
            } catch {
                // A failed capture may already have closed the stream.
            }
            if (created && !success) {
                try {
                    file.delete(null);
                } catch {
                    // It may not have been created.
                }
            }
            this.busy = false;
        }
    }
}

export default class PureShotExtension extends Extension {
    enable() {
        this._dockVisibility = installDockVisibility();
        this._service = new CaptureService();
        this._removeWindowCorners = typeof installWindowCorners === 'function'
            ? installWindowCorners(this._service, global) : null;
        this._previews = new Map();
        // Wayland clients cannot choose global coordinates. Place only trusted
        // PureShot previews/history from inside the compositor; pins keep their position.
        this._mapId = global.window_manager.connect('map', (_wm, actor) => {
            const window = actor.meta_window;
            if (!window)
                return;
            // Diagnostic only: remove once always-on-top is confirmed working.
            console.log(`PureShot map: title="${window.get_title()}" pid=${window.get_pid()} class="${window.get_wm_class()}"`);
            if (!['PureShot Preview', 'PureShot Pin', 'PureShot History'].includes(window.get_title()))
                return;
            try {
                const executable = this._service._executable(window.get_pid());
                console.log(`PureShot map: executable=${executable}`);
                if (!this._service._allowedExecutables().includes(executable)) {
                    console.log('PureShot map: executable is not in allowlist, skipping');
                    return;
                }
                // GTK keep-above is only a hint on Wayland. Apply the pin's
                // stacking state in Mutter after validating its owning process.
                if (window.get_title() === 'PureShot Pin') {
                    window.make_above();
                    console.log(`PureShot map: make_above() called, above=${window.is_above()}`);
                    return;
                }
                // Map can precede GTK/WebKit's final configure. Reposition
                // after every size change too, so the close button cannot end
                // up outside the work area when the initial frame was smaller.
                window.make_above();
                const place = () => {
                    const history = window.get_title() === 'PureShot History';
                    const work = history
                        ? global.display.get_monitor_geometry(window.get_monitor())
                        : window.get_work_area_current_monitor();
                    const frame = window.get_frame_rect();
                    const margin = history ? 0 : 24;
                    window.move_frame(false,
                        history ? work.x : Math.max(work.x, work.x + work.width - frame.width - margin),
                        Math.max(work.y, work.y + work.height - frame.height - margin));
                };
                if (!this._previews.has(window)) {
                    const sizeId = window.connect('size-changed', place);
                    const unmanagedId = window.connect('unmanaged', () => {
                        window.disconnect(sizeId);
                        this._previews.delete(window);
                    });
                    this._previews.set(window, [sizeId, unmanagedId]);
                }
                place();
            } catch (error) {
                console.warn(`PureShot preview positioning: ${error.message}`);
            }
        });
        this._object = Gio.DBusExportedObject.wrapJSObject(IFACE, this._service);
        this._object.export(Gio.DBus.session, OBJECT_PATH);
        this._nameId = Gio.bus_own_name_on_connection(
            Gio.DBus.session, BUS_NAME, Gio.BusNameOwnerFlags.NONE, null, null);

        // Registered with the compositor itself, so it keeps working under
        // Wayland even though PureShot is an unprivileged client application.
        this._settings = this.getSettings('org.gnome.shell.extensions.pureshot');
        for (const [key, mode] of Object.entries(SHORTCUT_KEYS)) {
            Main.wm.addKeybinding(
                key,
                this._settings,
                Meta.KeyBindingFlags.NONE,
                Shell.ActionMode.NORMAL,
                () => this._onShortcut(mode));
        }
    }

    _onShortcut(mode) {
        if (!this._service || !this._object || this._service.busy)
            return;
        try {
            this._service._checkSession();
        } catch (error) {
            console.warn(`PureShot shortcut ignored: ${error.message}`);
            return;
        }
        // The app owns capture + UI; the shortcut only needs to wake it up.
        this._object.emit_signal('ShortcutActivated', new GLib.Variant('(s)', [mode]));
    }

    disable() {
        this._removeWindowCorners?.();
        this._removeWindowCorners = null;
        this._dockVisibility?.clear();
        this._dockVisibility = null;
        if (this._settings) {
            for (const key of Object.keys(SHORTCUT_KEYS))
                Main.wm.removeKeybinding(key);
            this._settings = null;
        }
        for (const [window, signals] of this._previews ?? []) {
            for (const id of signals)
                window.disconnect(id);
        }
        this._previews?.clear();
        if (this._mapId) {
            global.window_manager.disconnect(this._mapId);
            this._mapId = 0;
        }
        if (this._service)
            this._service.stop();
        this._object?.unexport();
        if (this._nameId)
            Gio.bus_unown_name(this._nameId);
        this._service = null;
        this._object = null;
        this._nameId = 0;
    }
}
