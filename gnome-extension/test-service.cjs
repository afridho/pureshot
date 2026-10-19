// Evaluate the real service in a VM with security/file and compositor mocks.
// This tests sequencing, not the OS/Gio implementations of trust checks.
const {test} = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');

function fixture() {
    let nextId = 0;
    const signals = new Set();
    const emitter = {connect() { signals.add(++nextId); return nextId; }, disconnect(id) { signals.delete(id); }};
    const state = {captures: 0, closes: 0, deletes: 0, creates: 0, painted: false};
    const Main = {sessionMode: {...emitter}, layoutManager: {...emitter}};
    const Gio = {Settings: class {connect = emitter.connect; disconnect = emitter.disconnect;},
        FileQueryInfoFlags: {NOFOLLOW_SYMLINKS: 1}, FileCreateFlags: {PRIVATE: 1},
        BusNameWatcherFlags: {NONE: 0},
        bus_watch_name_on_connection() { signals.add(++nextId); return nextId; },
        bus_unwatch_name(id) { signals.delete(id); }};
    const GLib = {Variant: class {constructor(_signature, values) { this.values = values; }}};
    const Shell = {Screenshot: class {
        screenshot_area(...args) {
            assert.equal(state.painted, true);
            state.captures++;
            args.at(-1)(this, {});
        }
        screenshot_area_finish() { return [true]; }
        screenshot_window(frame, cursor, _stream, callback) {
            assert.equal(state.painted, true);
            assert.equal(frame, true);
            assert.equal(cursor, false);
            state.captures++;
            callback(this, {});
        }
        screenshot_window_finish() { return [true]; }
    }};
    const context = vm.createContext({Gio, GLib, Main, Shell, Extension: class {}});
    const source = fs.readFileSync(__dirname + '/extension.js', 'utf8')
        .replace(/^import .*;\n/gm, '').replace('export default class', 'class');
    vm.runInContext(source + '\nglobalThis.Service = CaptureService; globalThis.PureShot = PureShotExtension;', context);
    const service = new context.Service();
    service._senderPid = async () => 42;
    service._executable = () => '/trusted';
    service._allowedExecutables = () => ['/trusted'];
    service._checkSession = () => { if (!service.active) throw new Error('disabled'); };
    service._outputFile = () => ({
        create() { state.creates++; return {close() { state.closes++; }}; },
        get_path() { return '/private/capture.png'; }, delete() { state.deletes++; },
    });
    service._afterPaint = async () => { state.painted = true; };
    function select(result) {
        context.result = result;
        context.state = state;
        vm.runInContext('LiveAreaSelection = class { constructor(radius) { state.radius = radius; } select() { return Promise.resolve(result); } finish() {} };', context);
    }
    const replies = [];
    const invocation = {get_connection() { return {}; }, get_sender() { return ':1.42'; },
        return_value(value) { replies.push(value.values[0]); },
        return_dbus_error(name) { replies.push(name); }};
    return {service, invocation, replies, select, state, signals, context};
}

test('SelectArea captures only after paint and returns a private output', async () => {
    const f = fixture();
    f.select([10, 20, 30, 40]);
    await f.service.SelectAreaAsync([13], f.invocation);
    assert.deepEqual(f.replies, ['/private/capture.png']);
    assert.equal(f.state.captures, 1);
    assert.equal(f.state.radius, 13);
    assert.equal(f.state.creates, 1);
    assert.equal(f.state.deletes, 0);
    assert.equal(f.signals.size, 0);
    assert.equal(f.service.busy, false);
});

function selectWindow(f, window) {
    f.context.selectedWindow = window;
    f.context.global = {get_current_time: () => 123,
        display: {get_focus_window: () => f.state.focus}};
    vm.runInContext('LiveWindowSelection = class { select() { return Promise.resolve(selectedWindow); } finish() {} };', f.context);
}

test('window capture focuses the selected window and excludes cursor', async () => {
    const f = fixture();
    const window = {get_compositor_private: () => ({}), showing_on_its_workspace: () => true,
        activate() { f.state.focus = this; }};
    selectWindow(f, window);
    await f.service.SelectWindowWithOptionsAsync(['{}'], f.invocation);
    assert.deepEqual(f.replies, ['/private/capture.png']);
    assert.equal(f.state.captures, 1);
    assert.equal(f.signals.size, 0);
});

test('window cancellation creates no file and focus failure never captures another window', async () => {
    for (const window of [null, {get_compositor_private: () => ({}),
        showing_on_its_workspace: () => true, activate() {}}]) {
        const f = fixture();
        selectWindow(f, window);
        await f.service.SelectWindowWithOptionsAsync(['{}'], f.invocation);
        assert.deepEqual(f.replies, [window ? 'org.pureshot.Capture.Failed' : '']);
        assert.equal(f.state.captures, 0);
        assert.equal(f.state.deletes, window ? 1 : 0);
        assert.equal(f.signals.size, 0);
        assert.equal(f.service.busy, false);
    }
});

test('cancellation returns empty path without creating a screenshot', async () => {
    const f = fixture();
    f.select(null);
    await f.service.SelectAreaAsync([13], f.invocation);
    assert.deepEqual(f.replies, ['']);
    assert.equal(f.state.creates, 0);
    assert.equal(f.state.captures, 0);
    assert.equal(f.signals.size, 0);
});

test('unauthorized caller is denied before any selection or file creation', async () => {
    const f = fixture();
    f.service._allowedExecutables = () => [];
    await f.service.SelectAreaAsync([13], f.invocation);
    assert.deepEqual(f.replies, ['org.pureshot.Capture.AccessDenied']);
    assert.equal(f.state.creates, 0);
    assert.equal(f.service.busy, false);
});

test('revocation during selection fails closed, including cancellation', async () => {
    for (const rect of [null, [10, 20, 30, 40]]) {
        const f = fixture();
        f.select(rect);
        let checks = 0;
        f.service._allowedExecutables = () => ++checks === 1 ? ['/trusted'] : [];
        await f.service.SelectAreaAsync([13], f.invocation);
        assert.deepEqual(f.replies, ['org.pureshot.Capture.Failed']);
        assert.equal(f.state.creates, 0);
        assert.equal(f.signals.size, 0);
    }
});

test('reauthorization failure after writing deletes output and releases busy', async () => {
    const f = fixture();
    f.select([10, 20, 30, 40]);
    let checks = 0;
    f.service._allowedExecutables = () => ++checks < 3 ? ['/trusted'] : [];
    await f.service.SelectAreaAsync([13], f.invocation);
    assert.deepEqual(f.replies, ['org.pureshot.Capture.Failed']);
    assert.equal(f.state.captures, 1);
    assert.equal(f.state.deletes, 1);
    assert.equal(f.signals.size, 0);
    assert.equal(f.service.busy, false);
});

test('disable while awaiting repaint fails without creating output', async () => {
    const f = fixture();
    f.select([10, 20, 30, 40]);
    f.service._afterPaint = async () => { f.service.stop(); };
    await f.service.SelectAreaAsync([13], f.invocation);
    assert.deepEqual(f.replies, ['org.pureshot.Capture.Failed']);
    assert.equal(f.state.creates, 0);
    assert.equal(f.signals.size, 0);
    assert.equal(f.service.busy, false);
});

test('shortcuts are normal-mode only and ignored while busy, disabled or session-restricted', () => {
    const f = fixture();
    const source = fs.readFileSync(__dirname + '/extension.js', 'utf8');
    assert.match(source, /Shell\.ActionMode\.NORMAL,/);
    assert.doesNotMatch(source, /Shell\.ActionMode\.ALL/);
    const extension = new f.context.PureShot();
    const emitted = [];
    extension._service = f.service;
    extension._object = {emit_signal(name, value) { emitted.push([name, value.values[0]]); }};
    f.service.busy = true;
    extension._onShortcut('area');
    f.service.busy = false;
    f.service.active = false;
    extension._onShortcut('area');
    f.service.active = true;
    extension._onShortcut('window');
    extension._service = null;
    extension._onShortcut('fullscreen');
    assert.deepEqual(emitted, [['ShortcutActivated', 'window']]);
});

test('busy rejects requests even before authorization', async () => {
    const f = fixture();
    f.service.busy = true;
    f.service._senderPid = () => { throw new Error('must not authorize'); };
    await f.service.SelectAreaAsync([13], f.invocation);
    assert.deepEqual(f.replies, ['org.pureshot.Capture.Busy']);
});
