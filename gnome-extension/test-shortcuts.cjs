// Mapping/schema regressions; compositor behavior still needs a live GNOME session.
const {test} = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');

const source = fs.readFileSync(__dirname + '/extension.js', 'utf8');
const schema = fs.readFileSync(__dirname + '/schemas/org.gnome.shell.extensions.pureshot.gschema.xml', 'utf8');
const keys = vm.runInNewContext(`(${source.match(/const SHORTCUT_KEYS = (\{[\s\S]*?\});/)[1]})`);

test('every shortcut action has a matching schema key, including unassigned OCR', () => {
    assert.deepEqual({...keys}, {
        'capture-area-shortcut': 'area',
        'capture-window-shortcut': 'window',
        'capture-fullscreen-shortcut': 'fullscreen',
        'capture-ocr-shortcut': 'ocr',
    });
    for (const key of Object.keys(keys))
        assert.match(schema, new RegExp(`<key name="${key}" type="as">`));
    assert.match(schema, /<key name="capture-ocr-shortcut" type="as">\s*<default>\[\]<\/default>/);
});

test('bindings use GSettings directly so empty arrays disable without a default fallback', () => {
    const registrations = [];
    const activations = [];
    const settings = {values: Object.fromEntries(Object.keys(keys).map(key => [key, []]))};
    const extension = {_settings: settings, _onShortcut: mode => activations.push(mode)};
    const loop = source.match(/for \(const \[key, mode\] of Object\.entries\(SHORTCUT_KEYS\)\) \{[\s\S]*?\n        \}/)[0];
    vm.runInNewContext(`(function() { ${loop} }).call(extension)`, {
        extension, SHORTCUT_KEYS: keys,
        Main: {wm: {addKeybinding(key, actualSettings, _flags, mode, callback) {
            assert.equal(actualSettings, settings);
            assert.deepEqual(actualSettings.values[key], []);
            assert.equal(mode, 1);
            registrations.push(key);
            callback();
        }}},
        Meta: {KeyBindingFlags: {NONE: 0}}, Shell: {ActionMode: {NORMAL: 1}},
    });
    assert.deepEqual(registrations, Object.keys(keys));
    assert.deepEqual(activations, ['area', 'window', 'fullscreen', 'ocr']);
});
