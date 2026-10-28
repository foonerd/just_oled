# Changelog

All notable changes to Just OLED are recorded here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Versions follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.0] - 2026-10-28

- The large panels set their pages in larger type. A panel of 200 rows and more (the 320x240 and 240x240 TFTs; not the 1.14" in portrait) steps every role up one face, 16/24/32 px in place of 12/16/24, so the type keeps the size it has on the OLEDs, whose pixels are coarser, and the rows take the room; the volume and connection icons and the level bars double with it, the progress line is three pixels. The OLED classes draw exactly as before, every picture of theirs byte for byte.
- A standby clock in 64-pixel digits wherever they fit the width, in place of a doubled 24-pixel face; a 12-hour clock's am/pm sits beside the digits on their baseline in the medium face. The 128-pixel-wide panels keep their way, since the digits do not fit there.
- Two faces join the binary: `evo_32` (the same Noto Sans build and glyph set as the others, 43 KB of bitmaps) and `evo_64` (the digits, colon, dot, dash and slash, 2 KB).
- `--render` draws the `320x240-rgb` class; the wiki's Screens page has its sheets.
## [0.1.9] - 2026-10-28

- A daemon started on its own gets the player's state. The plugin asks the daemon who it is every fifteen seconds (`--ping` now answers with the process id); a daemon it has not seen before, after an update, a crash or a hand restart, is given the player's state fetched from Volumio at that moment, so the position is current, rather than the boot rows and then the clock until the player's next change. The plugin also asks for the state whenever its own link to Volumio connects again.
## [0.1.8] - 2026-10-28

- After a plugin update the display comes back on its own. Volumio stops a plugin before replacing its files and does not start the new code until the player restarts; the installer now starts the display service itself when the plugin was on, so the panel is dark only for the seconds the update takes. A fresh install is unchanged: the service starts when the plugin is enabled.
- The settings page saves what it declares. Each section's keys are read from `UIConfig.json` when Save is pressed rather than from a list in the code, and a value whose kind the code does not know is stored by its shape; the previous version's code, which Volumio keeps in memory until the restart, then keeps a control the update added instead of dropping it.
## [0.1.7] - 2026-10-28

- The SPI TFT LCDs join the catalogue: ST7789 at 240x320 (the Waveshare 2", Adafruit's 2.0", Pimoroni's Display HAT Mini), 240x240 (the 1.3" squares, Pirate Audio), 135x240 (1.14") and 170x320 (1.9"); ST7735 at 128x160 and 80x160; ILI9341 at 240x320. RGB565 like the SSD1351, at 40 MHz (15 MHz for the ST7735); a 240x320 frame goes in 36 ms. Each glass sits where its data sheet puts it in the controller's RAM, with the colour order and inversion the module needs. The daemon sends the rows that changed between two frames rather than the whole frame, so the meter moves at its 25 fps on the large glasses too.
- A backlight pin for the TFTs: *Backlight GPIO* in the Connection section (18 in the generic and Waveshare wiring, where Waveshare's demos put it). The daemon drives it with the display: lit after the start-up sequence, dark in standby. -1 for a module whose backlight is tied to 3.3 V, and -1 for *Reset GPIO* on a module without one.
- *Orientation* takes quarter turns, 90 and 270, for the TFTs, which turn in the controller: a portrait module shows the landscape pages. An OLED turns half way as before; a quarter turn there shows as normal.
- An *SPI, Pimoroni HAT wiring* preset: CE1, DC on GPIO 9, backlight on GPIO 13, no reset line, as the Pirate Audio and the Display HAT Mini are wired.
- `--probe` names the backlight pin on a TFT; `--test` prints the frame's size after the turn.
## [0.1.6] - 2026-10-26

- The plugin's author is Just a Nerd in the store listing, as in the other plugins; the GitHub login stood there before.

## [0.1.5] - 2026-10-26

- The settings page in German, French, Italian, Dutch, Spanish, Polish, Japanese and Chinese, following the player's language; any other language shows English. A language file is all or nothing, since Volumio takes the plugin's toasts and modals from the one file of the player's language without falling back to the English one; the check refuses a file that lacks a key, a placeholder or the `|` separator.

## [0.1.4] - 2026-10-26

- A pause becomes standby. Volumio's play button pauses rather than stops, so a paused player is the usual resting state, and the pause text stayed on the panel until the player was stopped outright. Two settings at the head of the Standby section: *Pause goes to standby* (on) and *Standby after a pause (seconds)* (0, at once; up to an hour). The standby page then shows, and the display goes dark on the same schedule as after a stop. Off keeps the pause text until the player stops or plays.
- The level mapping goes through `ln` rather than `log10`, so a daemon built on a glibc 2.43 host no longer binds `log10f`'s new symbol version and the glibc gate passes everywhere.
- The plugin requires Volumio 4 (`engines.volumio`).

## [0.1.3] - 2026-10-16

- *I2C bus speed* in the Connection section: 100 kHz (the Pi's default), 400 kHz (what the controllers are specified for) or 1 MHz (beyond it; many modules take it). At 100 kHz a 128x64 frame takes about 90 ms and the meter moves at 10 frames a second; at 400 kHz it moves at the daemon's 25. Written to `/boot/userconfig.txt` between the plugin's marks for the hardware buses (`i2c_arm_baudrate`, `i2c_vc_baudrate`); a restart is asked for. `--probe` prints each bus's clock.
- *Meter across the panel* in the Face section: the meter takes the full width and at least the upper half, the rows give way from the top (album, artist, the artist · album row, then the format text, the larger title, the gaps) until the rest fits. On a 128x64 panel that is 32 bars over the time and the title, mpd_oled's own arrangement; on a 256x64, 64 bars.
- `ssd1309_128x64` verified on a 2.42" module over SPI (the first one-bit controller on the bench): the test picture to all four corners, the playing page at 25 fps, 1.1 ms a frame. Then over I2C at 0x3C on the header bus (the first I2C panel): 10.7 fps at 95 ms a frame, the 100 kHz bus's ceiling.

## [0.1.2] - 2026-10-16

- Colours for the colour panels: a *Colours* choice in the Face section with Classic (white, green bars with an amber top, red peak), Amber, Ice, Fire and White, or Custom with a colour picker for each of the title, the detail rows, the bars, the bars' top, the peak marks and the progress line. The boot, ready, pause and farewell texts take the theme too. A monochrome or greyscale panel keeps its own levels.
- Beside the time, the format text keeps to the volume and connection icons rather than floating mid-row.

## [0.1.1] - 2026-10-16

- The meter takes the room the rows leave. Turning off the sample rate and bit depth, or the track time, no longer leaves an empty row: the remaining items pack into one row, or into the end of the title row when only the volume and connection icons are left, and the meter grows by what is freed. A new choice, *Track text*, sets how many rows the track takes under the meter: compact (the default: the title under one row of artist and album), full (the title larger, the artist and the album on rows of their own, as before) or the title alone. On a 128x128 panel the default meter is half as tall again as it was; with everything off it has four fifths of the panel. On a wide panel the info column beside the meter is only as wide as its rows need, so a 256x64 shows 40 bars by default and 64 with the title alone.

## [0.1.0] - 2026-10-16

The first release, a pre-release: everything below is in, one module (the Waveshare 1.5" RGB OLED, SSD1351, on a Raspberry Pi 4) has been seen working, the other controllers are driven from their data sheets and wait for a bench.

- The face: what the panel shows while music plays is the user's choice: the spectrum, the stereo spectrum (left rising, right hanging), the spectrum with left and right level bars, the level bars alone, the track alone in larger type, or the spectrum across the whole panel. With it: the track's time as played and total, played, remaining or off; the sample rate and bit depth (a stream's bitrate) beside the volume and the connection; peak marks on or off; and a meter gain of -12 to +12 dB, since the tap reads the music before the volume control and so shows the recording's level, which quiet and loud masters set 20 dB apart. The standby page shows the time and date in the user's formats (the clock and date lists lcd1602-vumeter offers), the player's address when wanted, and goes dark after a chosen number of minutes or never. The settings page puts the face first, then standby, then the connection, then the messages.
- The playing page in mpd_oled's manner: the sample rate and bit depth, a volume wedge, the connection and its strength, the time played and the track's length, the album, the artist and the title scrolling when they do not fit, a progress line; laid out for a strip, a 64-row panel and a tall one.
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
