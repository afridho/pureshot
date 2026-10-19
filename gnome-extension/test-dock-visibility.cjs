const {test} = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');

test('dock filter hides only PureShot, preserves favorites and restores methods', () => {
    const app = (id, wmClass = '') => ({get_id: () => id,
        get_windows: () => [{get_wm_class: () => wmClass}]});
    const other = app('org.example.Editor.desktop');
    const entries = [other, app('com.pureshot.app.desktop'),
        app('PureShot.desktop'), app('window:123', 'pureshot')];
    class AppSystem { get_running() { return entries; } }
    const savedFavorites = Object.fromEntries(entries.map(item => [item.get_id(), item]));
    const favorites = {getFavoriteMap: () => savedFavorites};
    class InjectionManager {
        constructor() { this.restore = []; }
        overrideMethod(object, key, wrap) {
            const original = object[key];
            object[key] = wrap(original);
            this.restore.push(() => { object[key] = original; });
        }
        clear() { this.restore.reverse().forEach(restore => restore()); }
    }
    const context = vm.createContext({InjectionManager, Shell: {AppSystem},
        AppFavorites: {getAppFavorites: () => favorites},
        Gio: {FileQueryInfoFlags: {NOFOLLOW_SYMLINKS: 1}}, Extension: class {}});
    const source = fs.readFileSync(__dirname + '/extension.js', 'utf8')
        .replace(/^import .*;\n/gm, '').replace('export default class', 'class');
    vm.runInContext(source + '\nglobalThis.install = installDockVisibility;', context);
    const injections = context.install();
    assert.deepEqual(Array.from(new AppSystem().get_running()), [other]);
    assert.deepEqual(Object.keys(favorites.getFavoriteMap()), [other.get_id()]);
    assert.equal(Object.keys(savedFavorites).length, 4);
    assert.equal(entries.length, 4);
    injections.clear();
    assert.equal(new AppSystem().get_running(), entries);
    assert.equal(favorites.getFavoriteMap(), savedFavorites);
});
