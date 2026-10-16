'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');

// index.js needs Volumio's modules; only its pure helpers are under test here.
const Module = require('module');
const original = Module._load;
Module._load = function (request, parent, isMain) {
    if (['kew', 'fs-extra', 'socket.io-client', 'v-conf'].includes(request)) {
        return {};
    }
    return original.call(this, request, parent, isMain);
};
const { parseI2cdetect, panelOptions, stateForDaemon } = require('../index');
Module._load = original;

test('the addresses in an i2cdetect table are read, the blanks and UU skipped', () => {
    const table = [
        '     0  1  2  3  4  5  6  7  8  9  a  b  c  d  e  f',
        '00:          -- -- -- -- -- -- -- -- -- -- -- -- --',
        '10: -- -- -- -- -- -- -- -- -- -- -- -- -- -- -- --',
        '20: -- -- -- -- -- -- -- 27 -- -- -- -- -- -- -- --',
        '30: -- -- -- -- -- -- -- -- -- -- -- -- 3c -- -- --',
        '40: -- -- -- -- -- -- -- -- -- -- -- -- -- -- -- --',
        '50: -- -- -- -- -- -- -- -- -- -- -- -- -- -- -- --',
        '60: 60 -- -- -- -- -- -- -- -- -- -- UU -- -- -- --',
        '70: -- -- -- -- -- -- -- --',
        '',
    ].join('\n');
    assert.deepEqual(parseI2cdetect(table), [0x27, 0x3c, 0x60]);
    assert.deepEqual(parseI2cdetect(''), []);
});

test('the panel select is built from the daemon catalogue, described entries marked, undriven ones left out', () => {
    const entries = [
        { id: 'ssd1306_128x64', controller: 'SSD1306', width: 128, height: 64, verified: 'verified', driven: true },
        { id: 'sh1106_128x64', controller: 'SH1106', width: 128, height: 64, verified: 'described', driven: true },
        { id: 'ch1120_128x128', controller: 'CH1120', width: 128, height: 128, verified: 'described', driven: false },
        { nope: true },
    ];
    assert.deepEqual(panelOptions(entries), [
        { value: 'ssd1306_128x64', label: 'SSD1306 128x64' },
        { value: 'sh1106_128x64', label: 'SH1106 128x64 (described)' },
    ]);
    assert.deepEqual(panelOptions(null), []);
});

test('a pushState is reduced to what the daemon draws, with safe types', () => {
    const out = stateForDaemon({ status: 'play', title: 'A', artist: null, volume: '42', mute: 'no', seek: 1200, duration: 300, extra: 1 });
    assert.equal(out.kind, 'state');
    assert.equal(out.status, 'play');
    assert.equal(out.artist, '');
    assert.equal(out.volume, 42);
    assert.equal(out.mute, false);
    assert.equal(out.seek, 1200);
    assert.equal(out.duration, 300);
    assert.equal('extra' in out, false);
});
