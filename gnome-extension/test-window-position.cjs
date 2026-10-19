// Geometry regression tests; actual Wayland placement still needs a live Shell session.
const {test} = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');

const source = fs.readFileSync(__dirname + '/extension.js', 'utf8');
const placeBody = source.match(/const place = \(\) => \{([\s\S]*?)\n                \};/)[1];

function position(title, work, frame) {
    let result;
    const window = {
        get_title: () => title,
        get_monitor: () => 0,
        get_work_area_current_monitor: () => work,
        get_frame_rect: () => frame,
        move_frame: (_user, x, y) => { result = [x, y]; },
    };
    vm.runInNewContext(`(() => {${placeBody}})()`, {
        window, global: {display: {get_monitor_geometry: () => work}},
    });
    return result;
}

test('History is included in the trusted window integration', () => {
    assert.match(source, /\['PureShot Preview', 'PureShot Pin', 'PureShot History'\]\.includes/);
});

test('History sits flush at the bottom of the work area', () => {
    assert.deepEqual(position('PureShot History',
        {x: 0, y: 32, width: 1920, height: 1048},
        {width: 1920, height: 346}), [0, 734]);
});

test('History respects secondary monitor offsets and configured frame height', () => {
    const work = {x: -1920, y: 100, width: 1920, height: 1000};
    assert.deepEqual(position('PureShot History', work, {width: 1920, height: 330}), [-1920, 770]);
    assert.deepEqual(position('PureShot History', work, {width: 1920, height: 400}), [-1920, 700]);
});

test('Preview keeps its bottom-right margin', () => {
    assert.deepEqual(position('PureShot Preview',
        {x: 0, y: 32, width: 1920, height: 1048},
        {width: 336, height: 210}), [1560, 846]);
});
