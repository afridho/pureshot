// Headless behavioral tests; mocks cannot validate GNOME rendering or GI bindings.
// Run: node --test gnome-extension/test-selection.cjs
const {test} = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');

function setup(scale = 1) {
    let nextId = 1;
    const signals = new Map();
    let grabs = 0;
    class Actor {
        constructor(props) { Object.assign(this, props); this.ids = []; this.children = []; }
        add_child(actor) { this.children.push(actor); }
        set_cursor_type(cursor) { this.cursor = cursor; }
        set_position(x, y) { this.x = x; this.y = y; }
        set_size(width, height) { this.width = width; this.height = height; }
        set_content_scaling_filters() {}
        set_content(content) { this.content = content; }
        connect(name, cb) { const id = nextId++; signals.set(id, {name, cb}); this.ids.push(id); return id; }
        disconnect(id) { signals.delete(id); }
        show() {}
        destroy() { this.destroyed = true; this.ids.forEach(id => signals.delete(id)); this.children.forEach(child => child.destroy()); }
        queue_redraw() {}
        queue_repaint() { this.repaints = (this.repaints ?? 0) + 1; }
        get_surface_size() { return [this.width, this.height]; }
    }
    const stage = new Actor({width: 1920, height: 1080});
    stage.get_capture_final_size = rect => [true,
        Math.round(rect.width * scale), Math.round(rect.height * scale), scale];
    stage.get_color_state = () => ({});
    stage.paint_to_content = () => ({});
    const Main = {uiGroup: new Actor(), pushModal() { grabs++; return {}; },
        popModal() { grabs--; }, sessionMode: new Actor(), layoutManager: new Actor()};
    const Clutter = {Actor, ScalingFilter: {NEAREST: 0}, PaintFlag: {NO_CURSORS: 1},
        CursorType: {CROSSHAIR: 1, NONE: 2}, EVENT_STOP: true, EVENT_PROPAGATE: false,
        EventType: {KEY_PRESS: 1, BUTTON_PRESS: 2, MOTION: 3, BUTTON_RELEASE: 4, ENTER: 5, LEAVE: 6},
        KEY_Escape: 27, BUTTON_PRIMARY: 1, BUTTON_SECONDARY: 3};
    const GLib = {timeout_add(_p, _ms, cb) { const id = nextId++; signals.set(id, {name: 'timeout', cb}); return id; },
        idle_add(_p, cb) { const id = nextId++; signals.set(id, {name: 'idle', cb}); return id; },
        source_remove(id) { signals.delete(id); }, SOURCE_REMOVE: false};
    const display = {set_cursor(cursor) { this.cursor = cursor; }};
    const context = vm.createContext({St: {Widget: Actor, DrawingArea: Actor, Label: Actor}, Clutter, Main,
        Meta: {Cursor: {CROSSHAIR: 1, DEFAULT: 0}, WindowType: {NORMAL: 0, DIALOG: 1, MODAL_DIALOG: 2, DESKTOP: 3}},
        global: {stage, display, get_pointer: () => [0, 0]},
        Mtk: {Rectangle: class {constructor(props) { Object.assign(this, props); }}},
        Cairo: {FillRule: {EVEN_ODD: 1}, LineJoin: {ROUND: 1}, LineCap: {ROUND: 1}},
        Gio: {FileQueryInfoFlags: {NOFOLLOW_SYMLINKS: 1}}, GLib, Extension: class {}});
    const source = fs.readFileSync(__dirname + '/extension.js', 'utf8')
        .replace(/^import .*;\n/gm, '').replace('export default class', 'class');
    vm.runInContext(source + '\nglobalThis.classes = {LiveAreaSelection, LiveWindowSelection, CaptureService};', context);
    function flushIdle() {
        for (const [id, source] of [...signals]) {
            if (source.name !== 'idle')
                continue;
            signals.delete(id);
            source.cb();
        }
    }
    function event(type, button, x, y, key) {
        const callback = [...signals.values()].find(s => s.name === 'captured-event').cb;
        return callback(null, {type: () => type, get_button: () => button,
            get_coords: () => [x, y], get_key_symbol: () => key});
    }
    return {context, signals, event, flushIdle, Clutter, Main, get grabs() { return grabs; }};
}

test('reverse drag captures on release, destroys actor and releases modal', async () => {
    const env = setup();
    const selection = new env.context.classes.LiveAreaSelection();
    const result = selection.select();
    assert.equal(env.grabs, 1);
    env.event(2, 1, 300, 250);
    env.event(3, 0, 20, 40);
    env.event(4, 1, 20, 40);
    env.flushIdle();
    assert.deepEqual(Array.from(await result), [20, 40, 280, 210]);
    assert.equal(env.grabs, 0);
    assert.equal(selection.actor.destroyed, true);
    assert.equal(env.signals.size, 0);
});

function windowFixture(env) {
    const windows = [];
    env.context.global.get_window_actors = () => windows.map(meta_window => ({meta_window}));
    env.context.global.display.sort_windows_by_stacking = windows => windows;
    function add(x, y, width, height, type = 0) {
        const window = {minimized: false, visible: true,
            is_fullscreen() { return this.fullscreen ?? false; },
            get_window_type: () => type, showing_on_its_workspace: () => true,
            get_frame_rect: () => ({x, y, width, height}),
            get_compositor_private() { return {is_visible: () => this.visible}; }};
        windows.push(window);
        return window;
    }
    return {windows, add};
}

test('window hover shade and outline share rounded corners; maximized/fullscreen stay square', () => {
    const env = setup(1.5);
    const f = windowFixture(env);
    const window = f.add(0, 0, 300, 200);
    const selection = new env.context.classes.LiveWindowSelection();
    selection._draw();
    assert.equal(selection.radius, 20); // logical pixels, not divided by monitor scale
    const calls = [];
    selection._paint(new Proxy({}, {get: (_target, name) => (...args) => calls.push([name, ...args])}));
    const shade = calls.slice(0, calls.findIndex(call => call[0] === 'fillPreserve'));
    assert.equal(shade.filter(call => call[0] === 'arc').length, 4);
    assert.equal(shade.some(call => call[0] === 'rectangle'), false);
    for (const state of ['fullscreen', 'maximizedHorizontally', 'maximizedVertically']) {
        window[state] = true;
        selection._draw();
        assert.equal(selection.radius, 0);
        window[state] = false;
    }
    window.get_frame_rect = () => ({x: 0, y: 0, width: 12, height: 8});
    selection._draw();
    assert.equal(selection.radius, 4);
    selection.pointer = [500, 500];
    selection._draw();
    assert.equal(selection.radius, 0);
});

test('window picker selects topmost visible app, ignores desktop and uses a camera cursor', async () => {
    const env = setup();
    const f = windowFixture(env);
    const back = f.add(0, 0, 800, 600);
    const front = f.add(100, 100, 300, 200, 1);
    f.add(0, 0, 1920, 1080, 3);
    const selection = new env.context.classes.LiveWindowSelection();
    const result = selection.select();
    assert.equal(selection.actor.cursor, env.Clutter.CursorType.NONE);
    assert.equal(selection.window, back);
    env.event(3, 0, 120, 130);
    assert.equal(selection.window, front);
    assert.deepEqual(Array.from(selection.rect), [100, 100, 300, 200]);
    const calls = [];
    selection._paint(new Proxy({}, {get: (_target, name) => (...args) => calls.push([name, ...args])}));
    assert.ok(calls.some(call => call[0] === 'arc' && call[1] === 12 && call[2] === 13 && call[3] === 3));
    env.event(2, 1, 120, 130);
    env.event(4, 1, 120, 130);
    env.flushIdle();
    assert.equal(await result, front);
    assert.equal(env.grabs, 0);
    assert.equal(env.signals.size, 0);
});

test('window picker does not select background, minimized windows or a different release target', async () => {
    const env = setup();
    const f = windowFixture(env);
    f.add(100, 100, 300, 200).minimized = true;
    const selection = new env.context.classes.LiveWindowSelection();
    const result = selection.select();
    env.event(2, 1, 120, 130);
    env.event(4, 1, 120, 130);
    assert.equal(selection.window, null);
    f.add(100, 100, 300, 200);
    env.event(2, 1, 120, 130);
    env.event(4, 1, 500, 500);
    assert.equal(selection._finishing, false);
    env.event(1, 0, 0, 0, 27);
    env.flushIdle();
    assert.equal(await result, null);
    assert.equal(env.signals.size, 0);
});

test('custom plus replaces the system cursor even with guides disabled and cleans up on cancellation', async () => {
    const env = setup();
    const selection = new env.context.classes.LiveAreaSelection(13,
        {show_crosshair: false, show_magnifier: false});
    const result = selection.select();
    assert.equal(selection.actor.cursor, env.Clutter.CursorType.NONE);
    env.event(3, 0, 120, 160);
    const calls = [];
    const cr = new Proxy({}, {get: (_target, name) => (...args) => calls.push([name, ...args])});
    selection._paint(cr);
    assert.deepEqual(calls.filter(call => ['moveTo', 'lineTo'].includes(call[0])), [
        ['moveTo', 108.5, 160.5], ['lineTo', 132.5, 160.5],
        ['moveTo', 120.5, 148.5], ['lineTo', 120.5, 172.5],
    ]);
    assert.deepEqual(calls.filter(call => call[0] === 'setLineWidth'), [
        ['setLineWidth', 3], ['setLineWidth', 1],
    ]);
    assert.equal(calls.some(call => call[0] === 'strokePreserve'), true);
    selection.finish();
    env.flushIdle();
    await result;
    assert.equal(selection.actor.destroyed, true);
});

for (const [name, cancel] of [
    ['Escape', env => env.event(1, 0, 0, 0, 27)],
    ['right click', env => env.event(2, 3, 0, 0)],
    ['external abort', (_env, selection) => selection.finish()],
]) {
    test(`${name} cancels with complete modal cleanup`, async () => {
        const env = setup();
        const selection = new env.context.classes.LiveAreaSelection();
        const result = selection.select();
        cancel(env, selection);
        assert.equal(selection.actor.destroyed, undefined);
        assert.equal(selection._finishId > 0, true);
        env.flushIdle();
        assert.equal(await result, null);
        selection.finish(); // idempotent
        assert.equal(env.grabs, 0);
        assert.equal(env.signals.size, 0);
    });
}

test('click alone does not capture; drag clamps to stage bounds', async () => {
    const env = setup();
    const selection = new env.context.classes.LiveAreaSelection();
    const result = selection.select();
    env.event(2, 1, 10, 10);
    env.event(4, 1, 10, 10);
    assert.equal(env.grabs, 1);
    env.event(2, 1, -10, -20);
    env.event(4, 1, 2000, 2000);
    env.flushIdle();
    assert.deepEqual(Array.from(await result), [0, 0, 1920, 1080]);
});

test('rounded selection shade and border share a clamped source-pixel radius', () => {
    for (const scale of [1, 1.25, 1.5, 2]) {
        const env = setup(scale);
        const selection = new env.context.classes.LiveAreaSelection(20);
        selection._draw([10, 20, 300, 200]);
        assert.equal(selection.radius, 20 / scale);
        const calls = [];
        const cr = new Proxy({}, {get: (_target, name) => (...args) => calls.push([name, ...args])});
        selection._paint(cr);
        assert.equal(calls.some(call => call[0] === 'rectangle'), false);
        const fill = calls.findIndex(call => call[0] === 'fill');
        const firstPath = calls.slice(0, calls.findIndex(call => call[0] === 'setSourceRGBA'));
        const borderStart = calls.findIndex((call, i) => i > fill && call[0] === 'newSubPath');
        const borderPath = calls.slice(borderStart, calls.findIndex((call, i) => i > borderStart && call[0] === 'setSourceRGBA'));
        assert.deepEqual(firstPath, borderPath);
        assert.equal(firstPath.filter(call => call[0] === 'arc').length, 4);
        assert.ok(calls.some(call => call[0] === 'setSourceRGBA' && call[4] === 0.15));
        selection._draw([0, 0, 2, 8]);
        assert.ok(selection.radius <= 1);
        assert.ok(selection.radius > 0);
    }
});

test('preview recomputes radius when capture scale changes and aborts safely on query failure', async () => {
    const env = setup(1);
    const selection = new env.context.classes.LiveAreaSelection(20);
    const result = selection.select();
    env.event(2, 1, 10, 20);
    env.event(3, 0, 310, 220);
    assert.equal(selection.radius, 20);
    env.context.global.stage.get_capture_final_size = () => [true, 600, 400, 2];
    env.event(3, 0, 310, 220);
    assert.equal(selection.radius, 10);
    env.context.global.stage.get_capture_final_size = () => [false, 0, 0, 0];
    env.event(3, 0, 310, 220);
    env.flushIdle();
    await assert.rejects(result, /capture scale/);
    assert.equal(env.grabs, 0);
    assert.equal(env.signals.size, 0);
});

test('zero radius shades a square selection; empty selection leaves desktop clear', () => {
    const env = setup(2);
    const selection = new env.context.classes.LiveAreaSelection(0);
    selection._draw([10, 20, 30, 40]);
    const calls = [];
    const cr = new Proxy({}, {get: (_target, name) => (...args) => calls.push([name, ...args])});
    selection._paint(cr);
    assert.equal(selection.radius, 0);
    assert.equal(calls.filter(call => call[0] === 'rectangle').length, 2);
    calls.length = 0;
    selection._draw([0, 0, 0, 0]);
    selection._paint(cr);
    assert.equal(calls.filter(call => call[0] === 'rectangle').length, 0);
    assert.equal(calls.some(call => call[0] === 'fill'), false);
});

test('crossing and unhandled events propagate; handled input stops without synchronous teardown', async () => {
    const env = setup();
    const selection = new env.context.classes.LiveAreaSelection();
    const result = selection.select();
    for (const args of [[5], [6], [99], [1, 0, 0, 0, 65], [2, 2], [4, 1]])
        assert.equal(env.event(...args), env.Clutter.EVENT_PROPAGATE);
    assert.equal(env.event(3, 0, 40, 50), env.Clutter.EVENT_STOP);
    assert.equal(env.event(1, 0, 0, 0, 27), env.Clutter.EVENT_STOP);
    assert.equal(selection.actor.destroyed, undefined);
    assert.equal(env.grabs, 1);
    const sourceId = selection._finishId;
    selection.finish([1, 2, 3, 4]);
    assert.equal(selection._finishId, sourceId);
    assert.equal(env.event(5), env.Clutter.EVENT_PROPAGATE);
    env.flushIdle();
    assert.equal(await result, null);
    assert.equal(selection._finishId, 0);
    assert.equal(env.signals.size, 0);
});

for (const failure of ['context', 'paint', 'dispose']) {
    test(`repaint ${failure} failure defers teardown until after Cairo disposal`, async () => {
        const env = setup();
        const selection = new env.context.classes.LiveAreaSelection();
        const result = selection.select();
        const order = [];
        const error = new Error(failure);
        selection.drawing.get_context = () => {
            if (failure === 'context') throw error;
            return {$dispose() {
                order.push('dispose');
                assert.equal(selection.actor.destroyed, undefined);
                if (failure === 'dispose') throw error;
            }};
        };
        selection._paint = () => { if (failure === 'paint') throw error; };
        const destroy = selection.actor.destroy.bind(selection.actor);
        selection.actor.destroy = () => { order.push('destroy'); destroy(); };
        const repaint = [...env.signals.values()].find(s => s.name === 'repaint').cb;
        repaint();
        assert.equal(selection.actor.destroyed, undefined);
        repaint(); // no second paint/context once failure has requested teardown
        env.flushIdle();
        await assert.rejects(result, e => e === error);
        assert.deepEqual(order, failure === 'context' ? ['destroy'] : ['dispose', 'destroy']);
        assert.equal(env.grabs, 0);
        assert.equal(env.signals.size, 0);
    });
}

for (const failure of ['disconnect', 'popModal', 'destroy', 'all']) {
    test(`cleanup continues and settles once when ${failure} throws`, async () => {
        const env = setup();
        const selection = new env.context.classes.LiveAreaSelection();
        const result = selection.select();
        const calls = [];
        for (const [object, method] of [[selection.actor, 'disconnect'],
            [env.Main, 'popModal'], [selection.actor, 'destroy']]) {
            const original = object[method].bind(object);
            object[method] = (...args) => {
                calls.push(method);
                original(...args);
                if (failure === method || failure === 'all') throw new Error(method);
            };
        }
        selection.finish();
        selection.finish();
        env.flushIdle();
        await assert.rejects(result, new RegExp(failure === 'all' ? 'disconnect' : failure));
        assert.deepEqual(calls, ['disconnect', 'popModal', 'destroy']);
        selection.finish();
        env.flushIdle();
        assert.equal(calls.length, 3);
        assert.equal(selection.grab, null);
        assert.equal(selection.resolve, null);
        assert.equal(selection.reject, null);
        assert.equal(selection._finishId, 0);
        assert.equal(env.signals.size, 0);
    });
}

test('setup failure rejects after deferred cleanup and preserves the original error', async () => {
    const env = setup();
    const selection = new env.context.classes.LiveAreaSelection();
    env.Main.pushModal = () => { throw new Error('push failed'); };
    selection.actor.destroy = () => { throw new Error('destroy failed'); };
    const result = selection.select();
    assert.equal(selection._finishId > 0, true);
    env.flushIdle();
    await assert.rejects(result, /push failed/);
    assert.equal(selection.resolve, null);
});

test('repaint wait disconnects stage and timeout on paint or interruption', async () => {
    for (const cancel of [false, true]) {
        const env = setup();
        const service = Object.create(env.context.classes.CaptureService.prototype);
        const result = service._afterPaint();
        assert.equal(env.signals.size, 2);
        if (cancel) {
            service._cancelWait();
            await assert.rejects(result, /interrupted/);
        } else {
            [...env.signals.values()].find(s => s.name === 'after-paint').cb();
            const [id, idle] = [...env.signals].find(([, s]) => s.name === 'idle');
            env.signals.delete(id); // GLib removes the dispatched one-shot source
            idle.cb();
            await result;
        }
        assert.equal(env.signals.size, 0);
        assert.equal(service._cancelWait, null);
    }
});
