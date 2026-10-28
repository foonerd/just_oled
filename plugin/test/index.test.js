'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');

// index.js needs Volumio's modules; only its pure helpers are under test here.
const Module = require('module');
const original = Module._load;
Module._load = function (request, parent, isMain) {
    if (['kew', 'socket.io-client', 'v-conf'].includes(request)) {
        return { resolve: () => ({}) };
    }
    if (request === 'fs-extra') {
        return require('node:fs');
    }
    return original.call(this, request, parent, isMain);
};
const JustOled = require('../index');
const { parseI2cdetect, panelOptions, stateForDaemon, sectionKeys } = JustOled;
Module._load = original;

// A plugin with a store and a command router that only remember what they were given.
function plugin() {
    const store = {};
    const p = Object.create(JustOled.prototype);
    p.config = { set: (k, v) => { store[k] = v; }, get: (k) => store[k] };
    p.logger = { error: () => { }, info: () => { } };
    p.commandRouter = { getI18nString: (k) => k, pushToastMessage: () => { } };
    p.reloadDaemon = () => { };
    p.applyBootConfig = () => ({ changed: false });
    return { p, store };
}

test('a section saves the keys UIConfig.json declares for it, by their shape', () => {
    assert.deepEqual(sectionKeys('saveMessages'), ['show_lifecycle', 'text_boot', 'text_ready', 'text_pause', 'text_shutdown', 'text_reboot']);
    assert.ok(sectionKeys('saveConnection').includes('gpio_bl'));
    assert.deepEqual(sectionKeys('saveNothing'), []);
    const { p, store } = plugin();
    p.saveMessages({ show_lifecycle: 'true', text_boot: 'Hi|there', text_ready: 'x', stray: 'no', text_pause: 'p' });
    assert.deepEqual(store, { show_lifecycle: true, text_boot: 'Hi|there', text_ready: 'x', text_pause: 'p' });
    // A select, a number and a switch not in the type lists still land by shape.
    const { p: q, store: s2 } = plugin();
    q.take({ a: { value: 'v', label: 'L' }, b: 7, c: false, d: 'text' }, 'a');
    q.take({ b: 7 }, 'b');
    q.take({ c: false }, 'c');
    q.take({ d: 'text' }, 'd');
    assert.deepEqual(s2, { a: 'v', b: 7, c: false, d: 'text' });
});

test('saving the connection writes the preset\'s pins over the form\'s', () => {
    const { p, store } = plugin();
    p.saveConnection({ connection: { value: 'spi_pirate' }, panel: { value: 'st7789_240x240' }, gpio_dc: 24, rotate: { value: '90' } });
    assert.equal(store.connection, 'spi_pirate');
    assert.equal(store.panel, 'st7789_240x240');
    assert.deepEqual([store.spi_cs, store.gpio_dc, store.gpio_reset, store.gpio_bl], [1, 9, -1, 13]);
    assert.equal(store.rotate, '90');
});

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
    const out = stateForDaemon({ status: 'play', title: 'A', artist: null, bitrate: '320 kbps', volume: '42', mute: 'no', seek: 1200, duration: 300, extra: 1 });
    assert.equal(out.kind, 'state');
    assert.equal(out.status, 'play');
    assert.equal(out.artist, '');
    assert.equal(out.bitrate, '320 kbps');
    assert.equal(out.volume, 42);
    assert.equal(out.mute, false);
    assert.equal(out.seek, 1200);
    assert.equal(out.duration, 300);
    assert.equal('extra' in out, false);
});

test('the settings page, the store and the strings agree', () => {
    const fs = require('node:fs');
    const path = require('node:path');
    const here = path.join(__dirname, '..');
    const ui = JSON.parse(fs.readFileSync(path.join(here, 'UIConfig.json'), 'utf8'));
    const store = JSON.parse(fs.readFileSync(path.join(here, 'config.json'), 'utf8'));
    const strings = JSON.parse(fs.readFileSync(path.join(here, 'i18n', 'strings_en.json'), 'utf8')).JUSTOLED;
    // The face comes first: it is what the user sees every day.
    assert.deepEqual(ui.sections.map((s) => s.id), ['section_face', 'section_standby', 'section_connection', 'section_messages']);
    for (const section of ui.sections) {
        for (const key of section.saveButton.data) {
            assert.ok(key in store, `${key} is saved by ${section.id} but not in config.json`);
            const element = section.content.find((e) => e.id === key);
            assert.ok(element, `${key} has no control in ${section.id}`);
            // The panel select is filled from the daemon's catalogue at runtime.
            if (element.element === 'select' && key !== 'panel') {
                const values = element.options.map((o) => String(o.value));
                assert.ok(values.includes(String(store[key].value)), `${key}'s default ${store[key].value} is not offered`);
                assert.ok(values.includes(String(element.value.value)), `${key}'s shown value is not offered`);
            }
        }
        for (const element of section.content) {
            for (const text of [element.label, element.doc, ...(element.options || []).map((o) => o.label)]) {
                if (typeof text === 'string' && text.startsWith('TRANSLATE.JUSTOLED.')) {
                    assert.ok(text.slice('TRANSLATE.JUSTOLED.'.length) in strings, `${text} has no English string`);
                }
            }
        }
    }
    // The clock and date formats are the daemon's lists, as strftime patterns.
    const pages = fs.readFileSync(path.join(here, '..', 'bins', 'just-oled', 'src', 'pages.rs'), 'utf8');
    const listed = (name) => {
        const m = pages.match(new RegExp(`pub const ${name}: &\\[&str\\] = &\\[([^\\]]*)\\]`));
        assert.ok(m, `${name} in pages.rs`);
        return [...m[1].matchAll(/"([^"]+)"/g)].map((x) => x[1]);
    };
    const select = (id) => ui.sections.flatMap((s) => s.content).find((e) => e.id === id).options.map((o) => o.value);
    assert.deepEqual(select('clock_format'), listed('CLOCK_FORMATS'));
    assert.deepEqual(select('date_format'), listed('DATE_FORMATS'));
    assert.deepEqual(select('theme'), listed('THEMES'));
    // The colour pickers write #rrggbb; the defaults are of that shape.
    for (const key of Object.keys(store).filter((k) => k.startsWith('colour_'))) {
        assert.match(store[key].value, /^#[0-9a-f]{6}$/, key);
    }
});
