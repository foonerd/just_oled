# Changelog

All notable changes to Just OLED are recorded here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Versions follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

- The audio tap: `libjustoledtap.so` (ALSA `type justoledtap`) passes every stream through byte for byte and measures it on a thread of its own, peak and RMS per channel and a spectrum bank of up to 256 log, mel or linear bands with a peak hold and onsets, DSD and DoP by density, into a ring under `/dev/shm`. The tap descends from glass's; the FFT and the ring layout are this project's own.
- The library's own JSON reader for Volumio's configuration store and its own radix-2 FFT, so the only external crate is `libc`.
- The workspace: the `justoled` library with the `tap` and `display` features, the `justoled-tap` ALSA plugin and the `just-oled` daemon, `libc` the only external crate, enforced by the check.
- The catalogue of controllers and panels the daemon will drive, with `just-oled --list` and `--list --json`.
- The Volumio plugin's shape: settings page (connection, messages), the ALSA contribution, the service unit, install and uninstall, the launcher.
- The build path: check, ship, package, the builder image, CI and release workflows.
