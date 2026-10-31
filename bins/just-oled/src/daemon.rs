//! The daemon proper: the panel opened and kept open, the plugin's
//! commands applied, the tap's ring followed while music plays, one frame
//! drawn per tick and sent only when it changed.
//!
//! Phases follow the player's life: `Boot` from the service's start
//! (before Volumio is up) until the plugin says `ready` or sends a state;
//! `Ready` holds the ready text a moment; `Player` shows what the player
//! does; `Message` and `Test` overlay for a while; `Farewell` is the last
//! picture before the system goes down, after which the panel is switched
//! off and left alone.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, RecvTimeoutError, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use justoled::canvas::Canvas;
use justoled::oled::Oled;
use justoled::ring::{self, merge_hops, Reader};

use crate::control::{Command, Farewell, Report, Status, Track};
use crate::meter::Bars;
use crate::net::{self, Net};
use crate::pages::{self, Marquee, View};
use crate::pattern;
use crate::settings::Settings;

/// Frames per second the loop aims at; the bus sets the ceiling.
const FPS: u64 = 25;
/// How long the boot text may stay before the player page takes over
/// without a word from the plugin.
const BOOT_PATIENCE: Duration = Duration::from_secs(90);
/// How long the ready text stays.
const READY_HOLD: Duration = Duration::from_millis(2500);
/// How often a missing panel is tried again.
const PANEL_RETRY: Duration = Duration::from_secs(5);
/// How often `/dev/shm` is looked at for a ring while there is none.
const RING_LOOK: Duration = Duration::from_millis(500);
/// Playing this long without a ring: the page says the tap is missing.
const NO_TAP_PATIENCE: Duration = Duration::from_secs(10);

#[derive(Clone, Debug, PartialEq)]
enum Phase {
    Boot,
    Ready { until: Instant },
    Player,
    Message { lines: Vec<String>, until: Instant },
    Test { until: Instant },
    Farewell,
}

impl Phase {
    fn name(&self) -> &'static str {
        match self {
            Phase::Boot => "boot",
            Phase::Ready { .. } => "ready",
            Phase::Player => "player",
            Phase::Message { .. } => "message",
            Phase::Test { .. } => "test",
            Phase::Farewell => "farewell",
        }
    }
}

pub fn log(line: &str) {
    eprintln!("just-oled: {line}");
}

/// The open panel with the frame last sent to it.
struct Panel {
    oled: Oled,
    canvas: Canvas,
    sent: Vec<u8>,
    /// Whether the controller is showing its RAM.
    on: bool,
    /// Frames sent since the last rate reading, and when that was.
    frames: u32,
    rate_since: Instant,
    /// Frames per second over the last reading, and the last frame's transfer time.
    pub fps: f32,
    pub frame_ms: f32,
}

impl Panel {
    fn open(settings: &Settings) -> Result<Panel, String> {
        let mut oled = Oled::open(settings.entry, &settings.wiring)
            .map_err(|e| format!("{}: {e}", settings.entry.id))?;
        let in_software = oled.orient(settings.rotate);
        oled.init()
            .map_err(|e| format!("{} init: {e}", settings.entry.id))?;
        let canvas = oled.canvas().with_rotate(in_software);
        Ok(Panel {
            oled,
            sent: Vec::new(),
            canvas,
            on: true,
            frames: 0,
            rate_since: Instant::now(),
            fps: 0.0,
            frame_ms: 0.0,
        })
    }

    /// Send the canvas if it differs from what the panel shows: the rows
    /// that changed where the controller takes a window, the whole frame
    /// otherwise.
    fn flush(&mut self) -> std::io::Result<()> {
        if self.sent.as_slice() == self.canvas.bytes() {
            return Ok(());
        }
        let started = Instant::now();
        let rows = self.changed_rows();
        self.oled.show_rows(&self.canvas, rows)?;
        self.frame_ms = started.elapsed().as_secs_f32() * 1000.0;
        self.sent.clear();
        self.sent.extend_from_slice(self.canvas.bytes());
        self.frames += 1;
        let span = started.duration_since(self.rate_since);
        if span >= Duration::from_secs(2) {
            self.fps = self.frames as f32 / span.as_secs_f32();
            self.frames = 0;
            self.rate_since = started;
        }
        Ok(())
    }

    fn display(&mut self, on: bool) -> std::io::Result<()> {
        if on == self.on {
            return Ok(());
        }
        if on {
            self.oled.on()?;
        } else {
            self.oled.off()?;
        }
        self.on = on;
        Ok(())
    }

    /// The first row through the last that differ from the frame sent; the
    /// whole frame when none was sent or the format is not row by row.
    fn changed_rows(&self) -> std::ops::Range<u16> {
        let h = self.canvas.height();
        let row_bytes = self.canvas.bytes().len() / h.max(1) as usize;
        if !self.oled.windowed() || self.sent.len() != self.canvas.bytes().len() || row_bytes == 0 {
            return 0..h;
        }
        let differs = |y: usize| {
            let span = y * row_bytes..(y + 1) * row_bytes;
            self.sent[span.clone()] != self.canvas.bytes()[span]
        };
        let first = (0..h as usize).find(|&y| differs(y)).unwrap_or(0);
        let last = (0..h as usize).rev().find(|&y| differs(y)).unwrap_or(0);
        first as u16..last as u16 + 1
    }
}

pub struct Daemon {
    /// None until the plugin's store exists and reads; the loop runs regardless.
    settings: Option<Settings>,
    settings_path: PathBuf,
    settings_error: Option<String>,
    ring_dir: PathBuf,
    panel: Option<Panel>,
    panel_error: Option<String>,
    panel_retry_at: Instant,
    ring: Option<Reader>,
    ring_seq: u64,
    ring_looked_at: Instant,
    bars: Bars,
    phase: Phase,
    track: Track,
    /// When the track's state arrived, to count the seek on from.
    track_at: Instant,
    status_since: Instant,
    started: Instant,
    album: Marquee,
    artist: Marquee,
    /// The artist and the album as one row.
    line: Marquee,
    title: Marquee,
    net: Net,
    net_read_at: Instant,
    pub report: Arc<Mutex<Report>>,
}

/// How often the connection and address are read again.
const NET_REFRESH: Duration = Duration::from_secs(10);

fn bars_for(settings: Option<&Settings>) -> Bars {
    let Some(s) = settings else {
        return Bars::new(32);
    };
    let (w, h) = s.size();
    Bars::new(pages::bar_count(w, h, &s.look)).with_gain_db(s.look.gain_db)
}

impl Daemon {
    pub fn new(settings: Option<Settings>, settings_path: PathBuf, ring_dir: PathBuf) -> Daemon {
        let now = Instant::now();
        let bars = bars_for(settings.as_ref());
        Daemon {
            settings,
            settings_path,
            settings_error: None,
            ring_dir,
            panel: None,
            panel_error: None,
            panel_retry_at: now,
            ring: None,
            ring_seq: 0,
            ring_looked_at: now - RING_LOOK,
            bars,
            phase: Phase::Boot,
            track: Track::default(),
            track_at: now,
            status_since: now,
            started: now,
            album: Marquee::default(),
            artist: Marquee::default(),
            line: Marquee::default(),
            title: Marquee::default(),
            net: Net::default(),
            net_read_at: now - NET_REFRESH,
            report: Arc::new(Mutex::new(Report::default())),
        }
    }

    fn status(&self) -> Status {
        self.track.status
    }

    /// Seconds into the track: the last seek the player sent, counted on
    /// while it plays.
    fn elapsed_s(&self, now: Instant) -> i64 {
        let mut ms = self.track.seek;
        if self.track.status == Status::Play {
            ms += now.duration_since(self.track_at).as_millis() as i64;
        }
        ms / 1000
    }

    /// The connection and address, read again every so often.
    fn refresh_net(&mut self, now: Instant) {
        if now.duration_since(self.net_read_at) >= NET_REFRESH {
            self.net = net::read();
            self.net_read_at = now;
        }
    }

    /// Open the panel when there is none, no more often than the retry
    /// allows: at early boot the device nodes may not be there yet, and
    /// before the plugin is turned on there are no settings to open it by.
    fn ensure_panel(&mut self, now: Instant) {
        if self.panel.is_some() || now < self.panel_retry_at {
            return;
        }
        self.panel_retry_at = now + PANEL_RETRY;
        if self.settings.is_none() {
            self.reload();
        }
        let Some(settings) = self.settings.as_ref() else {
            return;
        };
        match Panel::open(settings) {
            Ok(p) => {
                let (w, h) = settings.size();
                log(&format!(
                    "display: {} ({w}x{h}, {:?}) via {:?}",
                    settings.entry.id, settings.rotate, settings.wiring
                ));
                self.panel = Some(p);
                self.panel_error = None;
            }
            Err(e) => {
                if self.panel_error.as_deref() != Some(&e) {
                    log(&format!(
                        "no display: {e}; trying again every {}s",
                        PANEL_RETRY.as_secs()
                    ));
                }
                self.panel_error = Some(e);
            }
        }
    }

    /// A write that failed: the panel is dropped and opened again later.
    fn lost_panel(&mut self, err: std::io::Error) {
        log(&format!("display lost: {err}"));
        self.panel = None;
        self.panel_error = Some(err.to_string());
        self.panel_retry_at = Instant::now() + PANEL_RETRY;
    }

    /// The hops since the last look into the bars; nothing lets them fall.
    fn follow_ring(&mut self, now: Instant, dt: Duration) {
        let live = self.ring.as_ref().is_some_and(Reader::is_live);
        if !live && now.duration_since(self.ring_looked_at) >= RING_LOOK {
            self.ring_looked_at = now;
            self.ring = Reader::open_live(&self.ring_dir);
            if let Some(r) = &self.ring {
                // From the latest hop on: what came before this look is not replayed.
                self.ring_seq = r.info().seq.saturating_sub(1);
                log(&format!("ring: {}", r.path().display()));
            }
        }
        let merged = self
            .ring
            .as_ref()
            .filter(|r| r.is_live())
            .and_then(|r| merge_hops(r.since(self.ring_seq).into_iter()));
        match merged {
            Some(frame) => {
                self.ring_seq = frame.seq;
                self.bars.feed(&frame, dt);
            }
            None => self.bars.decay(dt),
        }
    }

    fn ring_live(&self) -> bool {
        self.ring.as_ref().is_some_and(Reader::is_live)
    }

    fn apply(&mut self, cmd: Command, now: Instant) {
        match cmd {
            Command::State(track) => {
                if track.status != self.track.status {
                    self.status_since = now;
                    if track.status != Status::Play {
                        self.bars.reset();
                    }
                }
                self.track = *track;
                self.track_at = now;
                self.album.set(&self.track.album, now);
                self.artist.set(&self.track.artist, now);
                self.line.set(&pages::artist_album(&self.track), now);
                self.title.set(&self.track.title, now);
                if self.phase == Phase::Boot {
                    self.phase = Phase::Player;
                }
            }
            Command::Ready => {
                let show = self.settings.as_ref().is_none_or(|s| s.show_lifecycle);
                if self.phase == Phase::Boot && show {
                    self.phase = Phase::Ready {
                        until: now + READY_HOLD,
                    };
                } else if self.phase == Phase::Boot {
                    self.phase = Phase::Player;
                }
            }
            Command::Message { lines, hold } => {
                if self.phase != Phase::Farewell {
                    self.phase = Phase::Message {
                        lines,
                        until: now + hold,
                    };
                }
            }
            Command::Test { hold } => {
                if self.phase != Phase::Farewell {
                    self.phase = Phase::Test { until: now + hold };
                }
            }
            Command::Farewell(kind) => self.farewell(kind),
            Command::Reload => self.reload(),
        }
    }

    /// The last picture before the system goes down; it stays, then the
    /// panel is switched off so nothing is left glowing on a dead player.
    fn farewell(&mut self, kind: Farewell) {
        self.phase = Phase::Farewell;
        let Some(s) = self.settings.as_ref() else {
            return;
        };
        let text = match kind {
            Farewell::Shutdown => s.text_shutdown.clone(),
            Farewell::Reboot => s.text_reboot.clone(),
        };
        let show = s.show_lifecycle;
        if let Some(p) = self.panel.as_mut() {
            if show {
                pages::lines(&mut p.canvas, &text, &s.look.theme);
            } else {
                p.canvas.clear();
            }
            let result = p.flush().and_then(|_| p.display(show));
            if let Err(e) = result {
                self.lost_panel(e);
            }
        }
    }

    /// Read the settings again; a file that is missing or will not read
    /// is said once and the old settings, if any, are kept.
    pub fn reload(&mut self) {
        let fresh = match std::fs::read_to_string(&self.settings_path)
            .map_err(|e| e.to_string())
            .and_then(|t| Settings::parse(&t).map_err(|e| e.to_string()))
        {
            Ok(s) => s,
            Err(e) => {
                if self.settings_error.as_deref() != Some(&e) {
                    log(&format!(
                        "settings {}: {e}; {}",
                        self.settings_path.display(),
                        if self.settings.is_some() {
                            "kept as they were"
                        } else {
                            "waiting for the plugin to be turned on"
                        }
                    ));
                }
                self.settings_error = Some(e);
                return;
            }
        };
        self.settings_error = None;
        let reopen = self
            .settings
            .as_ref()
            .is_none_or(|old| old.needs_reopen(&fresh));
        let rotate_changed = self
            .settings
            .as_ref()
            .is_some_and(|old| old.rotate != fresh.rotate);
        self.bars = bars_for(Some(&fresh));
        self.settings = Some(fresh);
        if reopen {
            if let Some(mut p) = self.panel.take() {
                p.canvas.clear();
                let _ = p.flush();
                let _ = p.display(false);
            }
            self.panel_retry_at = Instant::now();
            log("settings: read; the display is opened anew");
            return;
        }
        if rotate_changed {
            if let (Some(p), Some(s)) = (self.panel.as_mut(), self.settings.as_ref()) {
                p.canvas = p.oled.canvas().with_rotate(s.rotate.in_software());
                p.sent.clear();
            }
        }
        log("settings: reloaded");
    }

    /// One frame: what the panel shows now.
    fn draw(&mut self, now: Instant) {
        if self.phase == Phase::Farewell {
            return;
        }
        if self.phase == Phase::Boot && now.duration_since(self.started) > BOOT_PATIENCE {
            self.phase = Phase::Player;
        }
        if let Phase::Ready { until } | Phase::Message { until, .. } | Phase::Test { until } =
            self.phase
        {
            if now >= until {
                self.phase = Phase::Player;
            }
        }
        let ring_live = self.ring_live();
        self.refresh_net(now);
        let elapsed_s = self.elapsed_s(now);
        let (Some(panel), Some(s)) = (self.panel.as_mut(), self.settings.as_ref()) else {
            return;
        };
        let status = self.track.status;
        let since = now.duration_since(self.status_since);
        let blind = status == Status::Play && !ring_live && since > NO_TAP_PATIENCE;
        let mut display_on = true;
        let view = View {
            track: &self.track,
            bars: &self.bars,
            blind,
            elapsed_s,
            net: &self.net,
            look: &s.look,
            album: &self.album,
            artist: &self.artist,
            line: &self.line,
            title: &self.title,
            now,
        };
        // The standby page as the user set it: the clock and date in their
        // formats, or a dark panel; and dark after the timeout either way.
        let standby = |canvas: &mut Canvas, since: Duration| -> bool {
            if s.idle_clock {
                let clock = pages::local_strftime(&s.clock_format);
                let date = pages::local_strftime(&s.date_format);
                pages::standby(canvas, &clock, &date, &self.net, &s.look);
            } else {
                canvas.clear();
            }
            s.idle_clock && s.idle_timeout.is_none_or(|t| since < t)
        };
        let theme = &s.look.theme;
        match &self.phase {
            Phase::Boot if s.show_lifecycle => pages::lines(&mut panel.canvas, &s.text_boot, theme),
            Phase::Boot => {
                display_on = standby(&mut panel.canvas, Duration::ZERO);
            }
            Phase::Ready { .. } => pages::lines(&mut panel.canvas, &s.text_ready, theme),
            Phase::Message { lines, .. } => pages::lines(&mut panel.canvas, lines, theme),
            Phase::Test { .. } => pattern::draw(&mut panel.canvas),
            Phase::Farewell => {}
            Phase::Player if at_rest(status, since, s.pause_standby) => {
                display_on = standby(&mut panel.canvas, since);
            }
            Phase::Player if status == Status::Pause && s.show_lifecycle => {
                pages::lines(&mut panel.canvas, &s.text_pause, theme)
            }
            Phase::Player => pages::playing(&mut panel.canvas, &view),
        }
        let result = panel.flush().and_then(|_| panel.display(display_on));
        if let Err(e) = result {
            self.lost_panel(e);
        }
    }

    fn publish_report(&self) {
        if let Ok(mut r) = self.report.lock() {
            r.panel = self
                .panel
                .as_ref()
                .and(self.settings.as_ref())
                .map(|s| s.entry.id.to_string());
            r.panel_error = self
                .panel_error
                .clone()
                .or_else(|| self.settings_error.clone());
            r.ring = self.ring_live();
            r.phase = self.phase.name().to_string();
            r.status = self.status().name().to_string();
            r.fps = self.panel.as_ref().map_or(0.0, |p| p.fps);
            r.frame_ms = self.panel.as_ref().map_or(0.0, |p| p.frame_ms);
        }
    }

    /// Asked to leave: the farewell the plugin gave, else what systemd's
    /// queue says, else a dark panel.
    pub fn leave(&mut self) {
        if self.phase == Phase::Farewell {
            return;
        }
        match crate::lifecycle::pending_shutdown() {
            Some(kind) => {
                log(&format!("leaving: {kind:?}"));
                self.farewell(kind);
            }
            None => {
                log("leaving: the panel is cleared and switched off");
                if let Some(p) = self.panel.as_mut() {
                    p.canvas.clear();
                    let _ = p.flush();
                    let _ = p.display(false);
                }
            }
        }
    }

    pub fn run(&mut self, rx: Receiver<Command>) {
        let frame = Duration::from_millis(1000 / FPS);
        let mut last = Instant::now();
        loop {
            let now = Instant::now();
            let dt = now.duration_since(last);
            last = now;
            let signals = crate::lifecycle::take();
            if signals & crate::lifecycle::TERM != 0 {
                self.leave();
                return;
            }
            if signals & crate::lifecycle::HUP != 0 {
                self.reload();
            }
            self.ensure_panel(now);
            if self.status() == Status::Play && self.phase == Phase::Player {
                self.follow_ring(now, dt);
            } else {
                self.bars.decay(dt);
            }
            self.draw(now);
            self.publish_report();
            if !self.take_commands(&rx, now + frame) {
                return;
            }
        }
    }

    /// Apply what the plugin sent: everything already queued, then what
    /// arrives until `deadline`. The queue is drained first so a bus that
    /// takes longer than a frame to write (I2C at 100 kHz: some 95 ms for
    /// a 128x64 frame against the 40 ms frame) still has its commands
    /// applied once a frame, rather than only when a frame is unchanged
    /// and quick. Returns `false` when the sender is gone.
    fn take_commands(&mut self, rx: &Receiver<Command>, deadline: Instant) -> bool {
        loop {
            match rx.try_recv() {
                Ok(cmd) => self.apply(cmd, Instant::now()),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return false,
            }
        }
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return true;
            }
            match rx.recv_timeout(left) {
                Ok(cmd) => self.apply(cmd, Instant::now()),
                Err(RecvTimeoutError::Timeout) => return true,
                Err(RecvTimeoutError::Disconnected) => return false,
            }
        }
    }
}

/// Whether the player is at rest, so the standby page shows: stopped, or
/// paused for as long as makes a pause a stop. Volumio's play button
/// pauses rather than stops, so a pause is the usual resting state.
fn at_rest(status: Status, since: Duration, pause_standby: Option<Duration>) -> bool {
    match status {
        Status::Stop => true,
        Status::Pause => pause_standby.is_some_and(|t| since >= t),
        Status::Play => false,
    }
}

/// Where the tap's rings are.
pub fn ring_dir() -> PathBuf {
    Path::new(ring::DIR).to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;
    use justoled::canvas::Rotate;
    use justoled::catalog;
    use justoled::oled::Wiring;

    fn settings() -> Settings {
        Settings {
            entry: catalog::find("ssd1306_128x64").unwrap(),
            wiring: Wiring::I2c {
                bus: 200,
                addr: 0x3C,
                reset: None,
            },
            rotate: Default::default(),
            look: Default::default(),
            show_lifecycle: true,
            text_boot: vec!["Volumio".into(), "booting...".into()],
            text_ready: vec!["Volumio".into(), "ready".into()],
            text_pause: vec!["Paused".into()],
            text_shutdown: vec!["Bye".into()],
            text_reboot: vec!["Back soon".into()],
            idle_clock: true,
            idle_timeout: Some(Duration::from_secs(1800)),
            pause_standby: Some(Duration::ZERO),
            clock_format: "%H:%M".into(),
            date_format: "%a %d %b".into(),
        }
    }

    #[test]
    fn a_pause_is_a_stop_after_the_set_time_or_never() {
        let s = Duration::from_secs;
        assert!(at_rest(Status::Stop, s(0), None));
        assert!(!at_rest(Status::Play, s(3600), Some(s(0))));
        assert!(at_rest(Status::Pause, s(0), Some(s(0))), "at once");
        assert!(!at_rest(Status::Pause, s(29), Some(s(30))));
        assert!(at_rest(Status::Pause, s(30), Some(s(30))));
        assert!(!at_rest(Status::Pause, s(86400), None), "switched off");
    }

    #[test]
    fn phases_follow_the_commands_without_a_panel() {
        let dir = std::env::temp_dir().join(format!("just-oled-daemon-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let mut d = Daemon::new(Some(settings()), dir.join("none.json"), dir.clone());
        let now = Instant::now();
        assert_eq!(d.phase, Phase::Boot);
        d.ensure_panel(now);
        assert!(d.panel.is_none());
        assert!(
            d.panel_error.is_some(),
            "an undriven panel is reported, not retried blindly"
        );
        d.apply(Command::Ready, now);
        assert!(matches!(d.phase, Phase::Ready { .. }));
        d.draw(now + READY_HOLD + Duration::from_millis(1));
        assert_eq!(d.phase, Phase::Player);
        d.apply(
            Command::State(Box::new(Track {
                status: Status::Play,
                title: "x".into(),
                ..Default::default()
            })),
            now,
        );
        assert_eq!(d.status(), Status::Play);
        d.apply(
            Command::Message {
                lines: vec!["hi".into()],
                hold: Duration::from_secs(1),
            },
            now,
        );
        assert!(matches!(d.phase, Phase::Message { .. }));
        d.draw(now + Duration::from_secs(2));
        assert_eq!(d.phase, Phase::Player);
        d.apply(
            Command::Test {
                hold: Duration::from_secs(1),
            },
            now,
        );
        assert!(matches!(d.phase, Phase::Test { .. }));
        d.apply(Command::Farewell(Farewell::Reboot), now);
        assert_eq!(d.phase, Phase::Farewell);
        d.apply(
            Command::Test {
                hold: Duration::from_secs(1),
            },
            now,
        );
        assert_eq!(d.phase, Phase::Farewell, "nothing overlays the farewell");
        d.publish_report();
        let r = d.report.lock().unwrap();
        assert_eq!((r.phase.as_str(), r.status.as_str()), ("farewell", "play"));
        assert!(r.panel.is_none() && r.panel_error.is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn commands_are_taken_even_when_the_frame_left_no_time() {
        // A bus slower than the frame: the deadline is past before the
        // queue is looked at. Everything queued must still be applied.
        let mut d = Daemon::new(Some(settings()), PathBuf::from("/nonexistent"), ring_dir());
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send(Command::State(Box::new(Track {
            status: Status::Play,
            title: "Harvest Moon".into(),
            ..Default::default()
        })))
        .unwrap();
        tx.send(Command::Message {
            lines: vec!["hi".into()],
            hold: Duration::from_secs(1),
        })
        .unwrap();
        let past = Instant::now() - Duration::from_millis(100);
        assert!(d.take_commands(&rx, past));
        assert_eq!(d.track.title, "Harvest Moon");
        assert!(matches!(d.phase, Phase::Message { .. }));
        // With time left, a command arriving during the wait is taken too.
        let later = Instant::now() + Duration::from_millis(200);
        let tx2 = tx.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            tx2.send(Command::Farewell(Farewell::Shutdown)).unwrap();
        });
        assert!(d.take_commands(&rx, later));
        assert_eq!(d.phase, Phase::Farewell);
        // A sender that is gone ends the loop.
        drop(tx);
        assert!(!d.take_commands(&rx, Instant::now()));
    }

    #[test]
    fn a_reload_without_a_file_keeps_the_settings() {
        let mut d = Daemon::new(
            Some(settings()),
            PathBuf::from("/nonexistent/config.json"),
            ring_dir(),
        );
        d.reload();
        assert_eq!(d.settings.as_ref().unwrap().entry.id, "ssd1306_128x64");
        assert!(d.settings_error.is_some());
        // Without settings at all the loop still turns and says so once.
        let mut d = Daemon::new(None, PathBuf::from("/nonexistent/config.json"), ring_dir());
        d.ensure_panel(Instant::now());
        assert!(d.settings.is_none() && d.panel.is_none());
        d.draw(Instant::now());
        d.publish_report();
        assert!(d.report.lock().unwrap().panel_error.is_some());
        assert!(settings().needs_reopen(&Settings {
            wiring: Wiring::I2c {
                bus: 1,
                addr: 0x3D,
                reset: None
            },
            ..settings()
        }));
        assert!(!settings().needs_reopen(&Settings {
            show_lifecycle: false,
            ..settings()
        }));
        // An OLED turns in the canvas: no reopen, the size stays. A TFT
        // turns in its controller: reopened, the sides swap.
        let turned = Settings {
            rotate: Rotate::Quarter,
            ..settings()
        };
        assert!(!settings().needs_reopen(&turned));
        assert_eq!(turned.size(), (128, 64));
        let tft = |rotate: Rotate| Settings {
            entry: catalog::find("st7789_240x320").unwrap(),
            wiring: Wiring::Spi {
                bus: 0,
                cs: 0,
                dc: 25,
                reset: Some(27),
                backlight: Some(18),
            },
            rotate,
            ..settings()
        };
        assert!(tft(Rotate::Normal).needs_reopen(&tft(Rotate::Quarter)));
        assert!(!tft(Rotate::Half).needs_reopen(&tft(Rotate::Half)));
        assert_eq!(tft(Rotate::Normal).size(), (240, 320));
        assert_eq!(tft(Rotate::Quarter).size(), (320, 240));
        assert!(
            bars_for(Some(&tft(Rotate::Quarter))).count()
                > bars_for(Some(&tft(Rotate::Normal))).count()
        );
    }
}
