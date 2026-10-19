const {test} = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const source = fs.readFileSync(__dirname + '/window-corners.js', 'utf8');
const predicate = source.match(/export function shouldRoundWindow\(title\) \{([\s\S]*?)\n\}/)[1];
const shouldRound = title => vm.runInNewContext(`(() => {${predicate}})()`, {title});

test('only Settings and editors are rounded, never transparent capture surfaces', () => {
    for (const title of ['PureShot', 'PureShot Annotate', 'PureShot — Extract Text'])
        assert.equal(shouldRound(title), true);
    for (const title of ['PureShot Preview', 'PureShot Pin', 'PureShot History', 'Other App'])
        assert.equal(shouldRound(title), false);
});

test('rounding is executable-authorized and cleaned up on disable', () => {
    assert.match(source, /_allowedExecutables\(\)\.includes\(service\._executable/);
    assert.match(source, /actor\.remove_effect\(effect\)/);
    assert.match(source, /is_fullscreen\(\).*maximizedHorizontally.*maximizedVertically/);
});
