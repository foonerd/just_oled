# Changelog

All notable changes to Just OLED are recorded here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Versions follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

- The four-bit grey controllers: SSD1327 (1.5" 128x128, 128x96, 96x96), SSD1322 and SSD1362 (256x64), SSD1325 (128x64) and SH1122 (256x64, row by row), each glass at its place in the controller's RAM, the SSD1322 taking its arguments as data and the others as commands, transcribed from the data sheets, u8g2's drivers and luma.oled. Described, not yet drawn on a module. SPI clocks per controller: Sino Wealth's at 4 MHz, Solomon Systech's at 8, the SSD1351 at 10.
- The one-bit controllers: SSD1306 and its relatives (SSD1305, SSD1309, SSD1315) in one burst over the glass's window, SH1106 and SH1107 a page at a time, each glass at its own place in the controller's RAM (the 64-column glasses 32 columns in, the 1.3" SH1106 two columns in, the SH1107 64x128 at COM offset 0x60), transcribed from the data sheets, u8g2's drivers and luma.oled. Described, not yet drawn on a module. The SH1107 128x64 (a 64x128 on its side) waits for the quarter turn.
- The daemon: the panel opened from the plugin's settings and kept open, the lifecycle texts (boot, ready, pause, shutdown, restart) in the player's life, the now-playing page with the title, the artist and the spectrum bars from the tap's ring, a level bar for DSD, the standby clock, a test picture and messages on request, the control socket the plugin talks to, `--say`, `--ping`. Verified on a Waveshare 1.5inch RGB OLED (SSD1351) on a Raspberry Pi 4.
- The evo fonts: the evo framework's text face rasterised into one-bit glyph tables at three sizes (SIL Open Font License 1.1, `fonts/OFL-evo.txt`), drawn by the canvas with no font code on the player.
- The display path: the library speaks to `/dev/spidev*`, `/dev/i2c-*` and `/dev/gpiochip*` through their ioctls directly (no helper libraries), keeps the frame in the controller's own pixel layout (one-bit pages, four-bit grey, RGB565) with an upside-down option, and drives the SSD1351 from its data sheet and the module vendor's reference sequence. `just-oled --probe` lists the machine's buses and chips beside the settings; `just-oled --test` puts the test picture on the configured panel.
- The audio tap: `libjustoledtap.so` (ALSA `type justoledtap`) passes every stream through byte for byte and measures it on a thread of its own, peak and RMS per channel and a spectrum bank of up to 256 log, mel or linear bands with a peak hold and onsets, DSD and DoP by density, into a ring under `/dev/shm`. The tap descends from glass's; the FFT and the ring layout are this project's own.
- The library's own JSON reader for Volumio's configuration store and its own radix-2 FFT, so the only external crate is `libc`.
- The workspace: the `justoled` library with the `tap` and `display` features, the `justoled-tap` ALSA plugin and the `just-oled` daemon, `libc` the only external crate, enforced by the check.
- The catalogue of controllers and panels the daemon will drive, with `just-oled --list` and `--list --json`.
- The Volumio plugin's shape: settings page (connection, messages), the ALSA contribution, the service unit, install and uninstall, the launcher.
- The build path: check, ship, package, the builder image, CI and release workflows.
