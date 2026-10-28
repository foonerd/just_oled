'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const boot = require('../lib/boot');

test('the presets name the pins, custom leaves them alone', () => {
    assert.deepEqual(boot.preset('spi'), { bus: 'spi', spi_bus: 0, spi_cs: 0, gpio_dc: 24, gpio_reset: 25, gpio_bl: 18 });
    assert.deepEqual(boot.preset('spi_waveshare'), { bus: 'spi', spi_bus: 0, spi_cs: 0, gpio_dc: 25, gpio_reset: 27, gpio_bl: 18 });
    assert.deepEqual(boot.preset('spi_pirate'), { bus: 'spi', spi_bus: 0, spi_cs: 1, gpio_dc: 9, gpio_reset: -1, gpio_bl: 13 });
    assert.deepEqual(boot.preset('spi_custom'), { bus: 'spi' });
    assert.deepEqual(boot.preset('i2c_custom'), { bus: 'i2c' });
    assert.equal(boot.preset('nonsense').bus, 'i2c');
});

test('only a custom I2C bus other than 1 writes boot configuration', () => {
    assert.deepEqual(boot.linesFor('i2c', 1, 2, 3), []);
    assert.deepEqual(boot.linesFor('i2c_custom', 1, 2, 3), []);
    assert.deepEqual(boot.linesFor('i2c_custom', 0, 2, 3), ['dtparam=i2c_vc=on']);
    assert.deepEqual(boot.linesFor('i2c_custom', 3, 23, 24), ['dtoverlay=i2c-gpio,bus=3,i2c_gpio_sda=23,i2c_gpio_scl=24,i2c_gpio_delay_us=2']);
    assert.deepEqual(boot.linesFor('i2c_custom', 5, 99, -1), ['dtoverlay=i2c-gpio,bus=5,i2c_gpio_sda=2,i2c_gpio_scl=3,i2c_gpio_delay_us=2']);
    assert.deepEqual(boot.linesFor('spi_custom', 3, 23, 24), []);
});

test('a faster clock is written for the hardware buses only', () => {
    assert.deepEqual(boot.linesFor('i2c', 1, 2, 3, 400), ['dtparam=i2c_arm_baudrate=400000']);
    assert.deepEqual(boot.linesFor('i2c', 1, 2, 3, '1000'), ['dtparam=i2c_arm_baudrate=1000000']);
    assert.deepEqual(boot.linesFor('i2c', 1, 2, 3, 100), []);
    assert.deepEqual(boot.linesFor('i2c', 1, 2, 3, 250), [], 'only the two offered speeds');
    assert.deepEqual(boot.linesFor('i2c_custom', 0, 2, 3, 400), ['dtparam=i2c_vc=on', 'dtparam=i2c_vc_baudrate=400000']);
    assert.deepEqual(boot.linesFor('i2c_custom', 3, 23, 24, 400), ['dtoverlay=i2c-gpio,bus=3,i2c_gpio_sda=23,i2c_gpio_scl=24,i2c_gpio_delay_us=2']);
    assert.deepEqual(boot.linesFor('spi', 0, 0, 0, 400), []);
});

test('the block is replaced or removed without touching the rest of the file', () => {
    const before = 'dtparam=audio=on\n# >>> just_oled\nold\n# <<< just_oled\n';
    const replaced = boot.rewrite(before, ['new']);
    assert.equal(replaced.content, 'dtparam=audio=on\n# >>> just_oled\nnew\n# <<< just_oled\n');
    assert.equal(replaced.changed, true);
    const removed = boot.rewrite(before, []);
    assert.equal(removed.content, 'dtparam=audio=on\n');
    assert.equal(removed.changed, true);
    assert.equal(boot.rewrite('', []).changed, false);
    assert.equal(boot.rewrite('dtparam=audio=on\n', []).changed, false);
    assert.equal(boot.rewrite('dtparam=audio=on', ['x']).content, 'dtparam=audio=on\n# >>> just_oled\nx\n# <<< just_oled\n');
});
