# Changelog

All notable changes to Just OLED are recorded here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Versions follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

- The daemon: the panel opened from the plugin's settings and kept open, the lifecycle texts (boot, ready, pause, shutdown, restart) in the player's life, the now-playing page with the title, the artist and the spectrum bars from the tap's ring, a level bar for DSD, the standby clock, a test picture and messages on request, the control socket the plugin talks to, `--say`, `--ping`. Verified on a Waveshare 1.5inch RGB OLED (SSD1351) on a Raspberry Pi 4.
- The evo fonts: the evo framework's text face rasterised into one-bit glyph tables at three sizes (SIL Open Font License 1.1, `fonts/OFL-evo.txt`), drawn by the canvas with no font code on the player.
- The display path: the library speaks to `/dev/spidev*`, `/dev/i2c-*` and `/dev/gpiochip*` through their ioctls directly (no helper libraries), keeps the frame in the controller's own pixel layout (one-bit pages, four-bit grey, RGB565) with an upside-down option, and drives the SSD1351 from its data sheet and the module vendor's reference sequence. `just-oled --probe` lists the machine's buses and chips beside the settings; `just-oled --test` puts the test picture on the configured panel.
- The audio tap: `libjustoledtap.so` (ALSA `type justoledtap`) passes every stream through byte for byte and measures it on a thread of its own, peak and RMS per channel and a spectrum bank of up to 256 log, mel or linear bands with a peak hold and onsets, DSD and DoP by density, into a ring under `/dev/shm`. The tap descends from glass's; the FFT and the ring layout are this project's own.
- The library's own JSON reader for Volumio's configuration store and its own radix-2 FFT, so the only external crate is `libc`.
- The workspace: the `justoled` library with the `tap` and `display` features, the `justoled-tap` ALSA plugin and the `just-oled` daemon, `libc` the only external crate, enforced by the check.
- The catalogue of controllers and panels the daemon will drive, with `just-oled --list` and `--list --json`.
- The Volumio plugin's shape: settings page (connection, messages), the ALSA contribution, the service unit, install and uninstall, the launcher.
- The build path: check, ship, package, the builder image, CI and release workflows.
