//! What the panel shows: the lifecycle texts, the now-playing page in the
//! manner of mpd_oled (the bars, a status line with the stream's format,
//! the volume and the connection, the clock, the artist and the title
//! scrolling, a progress bar) and the standby page (address, clock,
//! date). Laid out by the panel's size class so one set of pages serves a
//! 128x32 strip and a 128x128 square alike.
//!
//! Colour panels get colour; a grey or one-bit panel gets the same shapes
//! in its own levels, the canvas folding the colours.

use std::time::{Duration, Instant};

use justoled::canvas::{Canvas, Colour};
use justoled::catalog::Format;
use justoled::font::{Font, LARGE, MEDIUM, SMALL};

use crate::control::Track;
use crate::meter::Bars;
use crate::net::{Connection, Net};

/// The colours of the pages, folded by the canvas on a monochrome panel.
struct Palette {
    title: Colour,
    detail: Colour,
    bar_low: Colour,
    bar_high: Colour,
    peak: Colour,
    progress: Colour,
}

fn palette(format: Format) -> Palette {
    match format {
        Format::Rgb565 => Palette {
            title: Colour::WHITE,
            detail: Colour::rgb(160, 160, 170),
            bar_low: Colour::rgb(0, 200, 120),
            bar_high: Colour::rgb(255, 180, 0),
            peak: Colour::rgb(255, 80, 60),
            progress: Colour::rgb(0, 150, 170),
        },
        _ => Palette {
            title: Colour::WHITE,
            detail: Colour::grey(200),
            bar_low: Colour::WHITE,
            bar_high: Colour::WHITE,
            peak: Colour::WHITE,
            progress: Colour::WHITE,
        },
    }
}

/// The font a panel's rows are set in: by height, so the rows fit.
pub fn font_for(height: u16) -> &'static Font {
    match height {
        0..=39 => SMALL,
        40..=95 => MEDIUM,
        _ => LARGE,
    }
}

/// The largest face, from the one the height allows down, in which every
/// row fits the width; the smallest when none does (the row is then cut).
fn font_that_fits<'a>(
    height: u16,
    width: i32,
    rows: impl Iterator<Item = &'a str> + Clone,
) -> &'static Font {
    let start = font_for(height);
    let ladder: &[&Font] = &[LARGE, MEDIUM, SMALL];
    // By name, not address: a `const` reference is not one address.
    let from = ladder
        .iter()
        .position(|f| f.name == start.name)
        .unwrap_or(0);
    for font in &ladder[from..] {
        if rows.clone().all(|r| font.width(r) <= width) {
            return font;
        }
    }
    SMALL
}

// -----------------------------------------------------------------------------
// Layout
// -----------------------------------------------------------------------------

/// A box on the panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

/// Where the parts of the playing page go on a panel of a given size.
#[derive(Clone, Copy, Debug)]
pub struct Layout {
    pub bars: Rect,
    /// The format, volume and connection row; `None` on a strip.
    pub status: Option<Rect>,
    /// The clock; `None` on a strip.
    pub clock: Option<(Rect, &'static Font)>,
    /// The artist row; `None` on a strip.
    pub artist: Option<(Rect, &'static Font)>,
    pub title: (Rect, &'static Font),
    pub progress: Rect,
}

const STATUS_H: i32 = 15;

/// The playing page's layout for a panel `w` by `h`:
/// - a strip (under 40 rows): the title over the bars, a progress line under;
/// - a 64-row panel: the bars on the left half, the status line and the
///   clock on the right, then artist and title across, as mpd_oled lays it;
/// - a tall panel: the bars across the top, then status, clock, artist, title.
pub fn layout(w: i32, h: i32) -> Layout {
    if h < 40 {
        let title_h = SMALL.line_height as i32;
        return Layout {
            bars: Rect {
                x: 0,
                y: title_h,
                w,
                h: h - title_h - 2,
            },
            status: None,
            clock: None,
            artist: None,
            title: (
                Rect {
                    x: 0,
                    y: 0,
                    w,
                    h: title_h,
                },
                SMALL,
            ),
            progress: Rect {
                x: 0,
                y: h - 1,
                w,
                h: 1,
            },
        };
    }
    let small = SMALL.line_height as i32;
    if h < 96 {
        let half = w / 2;
        let top_h = h / 2;
        return Layout {
            bars: Rect {
                x: 0,
                y: 0,
                w: half - 2,
                h: top_h,
            },
            status: Some(Rect {
                x: half + 2,
                y: 0,
                w: w - half - 2,
                h: STATUS_H,
            }),
            clock: Some((
                Rect {
                    x: half + 2,
                    y: STATUS_H - 2,
                    w: w - half - 2,
                    h: top_h - STATUS_H + 2,
                },
                MEDIUM,
            )),
            artist: Some((
                Rect {
                    x: 0,
                    y: top_h + 1,
                    w,
                    h: small,
                },
                SMALL,
            )),
            title: (
                Rect {
                    x: 0,
                    y: top_h + small,
                    w,
                    h: small,
                },
                SMALL,
            ),
            progress: Rect {
                x: 0,
                y: h - 1,
                w,
                h: 1,
            },
        };
    }
    // Tall: stacked. The title takes the medium face when the height allows.
    let title_font = if h >= 120 { MEDIUM } else { SMALL };
    let clock_h = MEDIUM.line_height as i32;
    let fixed = STATUS_H + clock_h + small + title_font.line_height as i32 + 2;
    let gaps = 4;
    let bars_h = h - fixed - gaps * 2;
    let mut y = 0;
    let bars = Rect {
        x: 0,
        y,
        w,
        h: bars_h,
    };
    y += bars_h + gaps;
    let status = Rect {
        x: 0,
        y,
        w,
        h: STATUS_H,
    };
    y += STATUS_H;
    let clock = Rect {
        x: 0,
        y,
        w,
        h: clock_h,
    };
    y += clock_h + gaps;
    let artist = Rect {
        x: 0,
        y,
        w,
        h: small,
    };
    y += small;
    let title = Rect {
        x: 0,
        y,
        w,
        h: title_font.line_height as i32,
    };
    Layout {
        bars,
        status: Some(status),
        clock: Some((clock, MEDIUM)),
        artist: Some((artist, SMALL)),
        title: (title, title_font),
        progress: Rect {
            x: 0,
            y: h - 2,
            w,
            h: 2,
        },
    }
}

/// How many bars the playing page shows on a panel: three pixels and a
/// gap each across the bars' box.
pub fn bar_count(width: u16, height: u16) -> usize {
    let bars = layout(width as i32, height as i32).bars;
    ((bars.w as usize + 1) / 4).clamp(4, 64)
}

// -----------------------------------------------------------------------------
// Scrolling rows
// -----------------------------------------------------------------------------

/// How long a row stands before it starts to scroll, and how fast it goes.
pub const SCROLL_DELAY: Duration = Duration::from_secs(5);
pub const SCROLL_PX_PER_S: f32 = 8.0;
/// The gap between the end of a scrolling text and its next start.
const SCROLL_GAP: i32 = 24;

/// A text row that scrolls when it is wider than its box: it stands for
/// the delay, runs left, and comes round again.
#[derive(Clone, Debug, Default)]
pub struct Marquee {
    text: String,
    since: Option<Instant>,
}

impl Marquee {
    /// A new text starts the clock again; the same text keeps scrolling.
    pub fn set(&mut self, text: &str, now: Instant) {
        if self.text != text {
            self.text = text.to_string();
            self.since = Some(now);
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    /// Pixels the text has moved left by `now`, for a text `text_w` wide
    /// in a box `box_w` wide; zero while it fits or stands.
    pub fn offset(&self, now: Instant, text_w: i32, box_w: i32) -> i32 {
        if text_w <= box_w {
            return 0;
        }
        let Some(since) = self.since else {
            return 0;
        };
        let moving = now.saturating_duration_since(since + SCROLL_DELAY);
        let travelled = (moving.as_secs_f32() * SCROLL_PX_PER_S) as i32;
        travelled % (text_w + SCROLL_GAP)
    }
}

/// A row of text in a box: left-aligned and still when it fits, scrolling
/// round when it does not, clipped to the box.
fn draw_marquee(
    canvas: &mut Canvas,
    font: &Font,
    rect: Rect,
    marquee: &Marquee,
    now: Instant,
    colour: Colour,
) {
    let text = marquee.text();
    if text.is_empty() {
        return;
    }
    let text_w = font.width(text);
    let offset = marquee.offset(now, text_w, rect.w - 2);
    canvas.set_clip(rect.x, rect.y, rect.w, rect.h);
    let x = rect.x + 1 - offset;
    canvas.text(font, x, rect.y, text, colour);
    if offset > 0 {
        canvas.text(font, x + text_w + SCROLL_GAP, rect.y, text, colour);
    }
    canvas.clear_clip();
}

// -----------------------------------------------------------------------------
// Pages
// -----------------------------------------------------------------------------

/// Rows of text centred on the panel, in the largest face that fits the
/// rows across; more rows than fit down are cut from the end.
pub fn lines(canvas: &mut Canvas, rows: &[String]) {
    canvas.clear();
    let w = canvas.width() as i32;
    let h = canvas.height() as i32;
    let font = font_that_fits(canvas.height(), w - 2, rows.iter().map(String::as_str));
    let line = font.line_height as i32;
    let shown = rows.len().min((h / line).max(1) as usize);
    let block = shown as i32 * line;
    let top = (h - block) / 2;
    let pal = palette(canvas.format());
    for (i, row) in rows.iter().take(shown).enumerate() {
        let colour = if i == 0 { pal.title } else { pal.detail };
        let fitted = font.fit(row, w - 2);
        canvas.text_centred(font, 0, top + i as i32 * line, w, &fitted, colour);
    }
}

/// Everything the playing page draws from.
pub struct View<'a> {
    pub track: &'a Track,
    pub bars: &'a Bars,
    /// No ring for a while: the bars' place says so.
    pub blind: bool,
    /// Into the track, in seconds, as the daemon counts it between states.
    pub elapsed_s: i64,
    pub clock: (u32, u32),
    pub net: &'a Net,
    pub artist: &'a Marquee,
    pub title: &'a Marquee,
    pub now: Instant,
}

/// The now-playing page.
pub fn playing(canvas: &mut Canvas, view: &View) {
    canvas.clear();
    let w = canvas.width() as i32;
    let h = canvas.height() as i32;
    let pal = palette(canvas.format());
    let lay = layout(w, h);

    // The bars, or the reason there are none.
    if view.blind {
        let note = "no audio tap";
        let font = font_that_fits(lay.bars.h as u16 * 2, lay.bars.w - 2, std::iter::once(note));
        let fitted = font.fit(note, lay.bars.w - 2);
        canvas.text_centred(
            font,
            lay.bars.x,
            lay.bars.y + (lay.bars.h - font.line_height as i32).max(0) / 2,
            lay.bars.w,
            &fitted,
            pal.detail,
        );
    } else if view.bars.levels().iter().all(|&l| l == 0.0) && view.bars.peak_level() > 0.0 {
        // One-bit audio has no bank: the level alone is drawn, as one wide bar.
        draw_level(canvas, lay.bars, view.bars, &pal);
    } else {
        draw_bars(canvas, lay.bars, view.bars, &pal);
    }

    if let Some(status) = lay.status {
        draw_status(canvas, status, view, &pal);
    }
    if let Some((rect, font)) = lay.clock {
        let text = format!("{:02}:{:02}", view.clock.0, view.clock.1);
        let y = rect.y + (rect.h - font.line_height as i32) / 2;
        canvas.text_centred(font, rect.x, y, rect.w, &text, pal.title);
    }
    if let Some((rect, font)) = lay.artist {
        draw_marquee(canvas, font, rect, view.artist, view.now, pal.detail);
    }
    let (rect, font) = lay.title;
    if view.title.text().is_empty() {
        canvas.text(font, rect.x + 1, rect.y, "Playing", pal.title);
    } else {
        draw_marquee(canvas, font, rect, view.title, view.now, pal.title);
    }

    if view.track.duration > 0 {
        let done = (lay.progress.w as i64 * view.elapsed_s.clamp(0, view.track.duration)
            / view.track.duration) as i32;
        canvas.fill_rect(
            lay.progress.x,
            lay.progress.y,
            done,
            lay.progress.h,
            pal.progress,
        );
    }
}

/// The stream's format on the left, the volume and the connection on the right.
fn draw_status(canvas: &mut Canvas, rect: Rect, view: &View, pal: &Palette) {
    let icon_w = 12;
    let icon_y = rect.y + (rect.h - 8) / 2;
    let mut right = rect.x + rect.w;
    if view.net.connection != Connection::None {
        right -= icon_w;
        draw_connection(canvas, right, icon_y, view.net.connection, pal.detail);
        right -= 3;
    }
    right -= icon_w;
    draw_volume(
        canvas,
        right,
        icon_y,
        view.track.volume,
        view.track.mute,
        pal.detail,
    );
    let room = right - 3 - rect.x;
    let text = format_text(view.track, room);
    canvas.set_clip(rect.x, rect.y, room, rect.h);
    canvas.text(SMALL, rect.x, rect.y, &text, pal.detail);
    canvas.clear_clip();
}

/// "44.1k 16b" when it fits, "44.1k" when it does not; the bit rate for a
/// stream that reports no sample rate.
fn format_text(track: &Track, room: i32) -> String {
    let first = |s: &str, suffix: &str| {
        s.split_whitespace()
            .next()
            .map(|n| format!("{n}{suffix}"))
            .unwrap_or_default()
    };
    let khz = first(&track.samplerate, "k");
    let bits = first(&track.bitdepth, "b");
    let kbps = first(&track.bitrate, "k");
    let full = match (khz.is_empty(), bits.is_empty()) {
        (false, false) => format!("{khz} {bits}"),
        (false, true) => khz.clone(),
        (true, _) => kbps,
    };
    if SMALL.width(&full) <= room {
        full
    } else if SMALL.width(&khz) <= room {
        khz
    } else {
        // Too narrow for even the rate: the icons alone.
        String::new()
    }
}

/// mpd_oled's volume mark: a wedge, filled to the volume; outline alone when muted.
fn draw_volume(canvas: &mut Canvas, x: i32, y: i32, volume: i64, mute: bool, colour: Colour) {
    let w = 12;
    let h = 8;
    let filled = if mute {
        0
    } else {
        (w as i64 * volume.clamp(0, 100) / 100) as i32
    };
    for i in 0..w {
        let col_h = ((i + 1) * h / w).max(1);
        let top = y + h - col_h;
        if i < filled {
            canvas.vline(x + i, top, col_h, colour);
        } else {
            canvas.set(x + i, top, colour);
            canvas.set(x + i, y + h - 1, colour);
        }
    }
}

/// Four bars for Wi-Fi, lit to the quality; a plug for a cable.
fn draw_connection(canvas: &mut Canvas, x: i32, y: i32, connection: Connection, colour: Colour) {
    match connection {
        Connection::Wifi(quality) => {
            for i in 0..4 {
                let bar_h = 2 + i * 2;
                let bx = x + i * 3;
                if i < quality as i32 {
                    canvas.fill_rect(bx, y + 8 - bar_h, 2, bar_h, colour);
                } else {
                    canvas.set(bx, y + 7, colour);
                    canvas.set(bx + 1, y + 7, colour);
                }
            }
        }
        Connection::Ethernet => {
            canvas.rect(x + 1, y + 1, 10, 6, colour);
            canvas.fill_rect(x + 3, y + 5, 6, 2, colour);
            canvas.hline(x + 3, y + 3, 6, colour);
        }
        Connection::None => {}
    }
}

/// The RMS as a bar across the box and the peak as a mark, in decibels.
fn draw_level(canvas: &mut Canvas, r: Rect, bars: &Bars, pal: &Palette) {
    let bar_h = (r.h / 3).clamp(2, 12);
    let top = r.y + (r.h - bar_h) / 2;
    let rms = (crate::meter::height(bars.rms_level()) * r.w as f32).round() as i32;
    let peak = (crate::meter::height(bars.peak_level()) * r.w as f32).round() as i32;
    canvas.fill_rect(r.x, top, rms, bar_h, pal.bar_low);
    if peak > rms {
        canvas.fill_rect(r.x + peak - 1, top, 1, bar_h, pal.peak);
    }
}

/// The bars in a box: each `levels[i]` a column from the bottom, the peak
/// mark a line above it, a one-pixel gap between bars.
fn draw_bars(canvas: &mut Canvas, r: Rect, bars: &Bars, pal: &Palette) {
    let n = bars.count() as i32;
    if n == 0 || r.w < n {
        return;
    }
    let pitch = r.w / n;
    let bar_w = (pitch - 1).max(1);
    let left = r.x + (r.w - pitch * n) / 2;
    let colour_split = r.h * 7 / 10;
    for (i, (&level, &peak)) in bars.levels().iter().zip(bars.peaks()).enumerate() {
        let bx = left + i as i32 * pitch;
        let lit = (level * r.h as f32).round() as i32;
        if lit > 0 {
            let low = lit.min(colour_split);
            canvas.fill_rect(bx, r.y + r.h - low, bar_w, low, pal.bar_low);
            if lit > colour_split {
                canvas.fill_rect(bx, r.y + r.h - lit, bar_w, lit - colour_split, pal.bar_high);
            }
        }
        let mark = (peak * r.h as f32).round() as i32;
        if mark > 0 && mark > lit {
            canvas.fill_rect(bx, r.y + r.h - mark, bar_w, 1, pal.peak);
        }
    }
}

/// The standby page: the address and the connection along the top, the
/// time as large as the panel allows, the date below; a strip shows the
/// time alone.
pub fn standby(canvas: &mut Canvas, clock: (u32, u32), date: &str, net: &Net) {
    canvas.clear();
    let w = canvas.width() as i32;
    let h = canvas.height() as i32;
    let text = format!("{:02}:{:02}", clock.0, clock.1);
    let pal = palette(canvas.format());
    if h < 40 {
        let y = (h - MEDIUM.line_height as i32) / 2;
        canvas.text_centred(MEDIUM, 0, y, w, &text, pal.title);
        return;
    }
    let small = SMALL.line_height as i32;
    if !net.ip.is_empty() {
        canvas.text(SMALL, 1, 0, &net.ip, pal.detail);
    }
    if net.connection != Connection::None {
        draw_connection(canvas, w - 13, 3, net.connection, pal.detail);
    }
    // The face and its pixel doubling by what is left between the rows.
    let room = h - 2 * small;
    let (font, scale) = if room >= 62 {
        (LARGE, 2)
    } else if room >= 32 {
        (LARGE, 1)
    } else {
        (MEDIUM, 1)
    };
    let text_w = font.width(&text) * scale;
    let text_h = font.line_height as i32 * scale;
    canvas.text_scaled(
        font,
        (w - text_w) / 2,
        small + (room - text_h) / 2,
        &text,
        pal.title,
        scale,
    );
    let date_font = if h >= 120 { MEDIUM } else { SMALL };
    let fitted = date_font.fit(date, w - 2);
    canvas.text_centred(
        date_font,
        0,
        h - date_font.line_height as i32,
        w,
        &fitted,
        pal.detail,
    );
}

// -----------------------------------------------------------------------------
// Time
// -----------------------------------------------------------------------------

fn local_tm() -> libc::tm {
    let now = unsafe { libc::time(std::ptr::null_mut()) };
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe {
        libc::localtime_r(&now, &mut tm);
    }
    tm
}

/// The local hour and minute, through the C library's time zone handling.
pub fn local_time() -> (u32, u32) {
    let tm = local_tm();
    (tm.tm_hour as u32, tm.tm_min as u32)
}

/// Today as "Thu 16 Oct 2026", in English.
pub fn local_date() -> String {
    const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let tm = local_tm();
    format!(
        "{} {} {} {}",
        DAYS[(tm.tm_wday.clamp(0, 6)) as usize],
        tm.tm_mday,
        MONTHS[(tm.tm_mon.clamp(0, 11)) as usize],
        1900 + tm.tm_year
    )
}

extern "C" {
    // Not in the libc crate; POSIX, in every glibc.
    fn tzset();
}

/// Read the time zone once, so `local_time` follows `/etc/localtime`.
pub fn tzset_now() {
    unsafe {
        tzset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use justoled::canvas::Canvas;

    fn lit_rows(c: &Canvas) -> Vec<i32> {
        (0..c.height() as i32)
            .filter(|&y| (0..c.width() as i32).any(|x| c.get(x, y) != Colour::BLACK))
            .collect()
    }

    fn view<'a>(
        track: &'a Track,
        bars: &'a Bars,
        net: &'a Net,
        artist: &'a Marquee,
        title: &'a Marquee,
        blind: bool,
    ) -> View<'a> {
        View {
            track,
            bars,
            blind,
            elapsed_s: 83,
            clock: (20, 45),
            net,
            artist,
            title,
            now: Instant::now(),
        }
    }

    #[test]
    fn fonts_and_layouts_follow_the_panel() {
        assert_eq!(font_for(32).line_height, 15);
        assert_eq!(font_for(64).line_height, 20);
        assert_eq!(font_for(128).line_height, 30);
        // The 64-row panel halves: bars left, 15 of them on a 128 wide panel.
        let l = layout(128, 64);
        assert_eq!(
            l.bars,
            Rect {
                x: 0,
                y: 0,
                w: 62,
                h: 32
            }
        );
        assert!(l.status.is_some() && l.clock.is_some() && l.artist.is_some());
        assert_eq!(l.progress.y, 63);
        assert_eq!(bar_count(128, 64), 15);
        assert_eq!(bar_count(256, 64), 31);
        // A strip has no status line or clock; the bars take the width.
        let l = layout(128, 32);
        assert!(l.status.is_none() && l.clock.is_none() && l.artist.is_none());
        assert_eq!(bar_count(128, 32), 32);
        // A tall panel stacks everything and the title grows.
        let l = layout(128, 128);
        assert_eq!(l.bars.w, 128);
        assert_eq!(l.title.1.line_height, 20);
        assert!(l.title.0.y + l.title.0.h <= l.progress.y, "{l:?}");
        let l = layout(128, 96);
        assert!(l.bars.h >= 20, "{l:?}");
        assert!(l.title.0.y + l.title.0.h <= l.progress.y, "{l:?}");
    }

    #[test]
    fn text_rows_are_centred_and_cut_to_what_fits() {
        let mut c = Canvas::new(Format::Paged1, 128, 32);
        lines(&mut c, &["Volumio".into(), "booting...".into()]);
        let rows = lit_rows(&c);
        assert!(!rows.is_empty());
        assert!(*rows.first().unwrap() >= 1 && *rows.last().unwrap() <= 30);
        lines(&mut c, &["a".into(), "b".into(), "c".into()]);
        let mut c = Canvas::new(Format::Rgb565, 128, 128);
        lines(&mut c, &["Volumio".into(), "ready".into()]);
        let rows = lit_rows(&c);
        let (top, bottom) = (*rows.first().unwrap(), *rows.last().unwrap());
        assert!((top - (127 - bottom)).abs() <= 8, "{top} {bottom}");
    }

    #[test]
    fn a_marquee_stands_then_scrolls_round() {
        let mut m = Marquee::default();
        let t0 = Instant::now();
        m.set("A very long title indeed", t0);
        assert_eq!(m.offset(t0, 200, 100), 0, "stands at first");
        assert_eq!(
            m.offset(t0 + Duration::from_secs(4), 200, 100),
            0,
            "still standing"
        );
        let moved = m.offset(t0 + Duration::from_secs(7), 200, 100);
        assert!((15..=17).contains(&moved), "{moved}");
        assert_eq!(
            m.offset(t0 + Duration::from_secs(60), 50, 100),
            0,
            "fits: no scroll"
        );
        // The same text keeps its clock; a new one starts over.
        m.set("A very long title indeed", t0 + Duration::from_secs(9));
        assert!(m.offset(t0 + Duration::from_secs(9), 200, 100) > 0);
        m.set("Short", t0 + Duration::from_secs(9));
        assert_eq!(m.offset(t0 + Duration::from_secs(10), 200, 100), 0);
    }

    #[test]
    fn the_playing_page_has_its_parts() {
        let mut bars = Bars::new(bar_count(128, 64));
        let frame = justoled::ring::Frame {
            spectrum: [vec![1.0; 64], vec![1.0; 64]],
            hold: [vec![1.0; 64], vec![1.0; 64]],
            ..Default::default()
        };
        bars.feed(&frame, Duration::from_millis(50));
        let track = Track {
            title: "A Song".into(),
            artist: "Someone".into(),
            samplerate: "44.1 kHz".into(),
            bitdepth: "16 bit".into(),
            volume: 50,
            duration: 200,
            ..Default::default()
        };
        let net = Net {
            connection: Connection::Wifi(3),
            ip: "192.168.1.9".into(),
        };
        let (mut artist, mut title) = (Marquee::default(), Marquee::default());
        let now = Instant::now();
        artist.set(&track.artist, now);
        title.set(&track.title, now);
        let mut c = Canvas::new(Format::Rgb565, 128, 64);
        playing(&mut c, &view(&track, &bars, &net, &artist, &title, false));
        // Full bars reach the bottom of their box, on the left half only.
        assert_ne!(c.get(1, 31), Colour::BLACK);
        assert_eq!(c.get(1, 32), Colour::BLACK, "under the bars box");
        // The progress line: 83 of 200 seconds is 53 of 128 pixels.
        assert_ne!(c.get(50, 63), Colour::BLACK);
        assert_eq!(c.get(60, 63), Colour::BLACK);
        // Something in the status row's right end: the icons.
        assert!((116..128).any(|x| (0..8).any(|y| c.get(x, y) != Colour::BLACK)));
        // And the clock in the right half.
        assert!((66..126).any(|x| (14..32).any(|y| c.get(x, y) != Colour::BLACK)));

        let mut c = Canvas::new(Format::Paged1, 128, 32);
        playing(&mut c, &view(&track, &bars, &net, &artist, &title, true));
        assert!(
            lit_rows(&c).iter().any(|&y| y > 16),
            "the blind note is drawn"
        );

        let mut dsd = Bars::new(32);
        dsd.feed(
            &justoled::ring::Frame {
                one_bit: true,
                peak: [0.5, 0.5],
                rms: [0.1, 0.1],
                ..Default::default()
            },
            Duration::from_millis(50),
        );
        let mut c = Canvas::new(Format::Paged1, 128, 32);
        playing(&mut c, &view(&track, &dsd, &net, &artist, &title, false));
        assert!(
            lit_rows(&c).iter().any(|&y| (16..30).contains(&y)),
            "the level bar is drawn"
        );
    }

    #[test]
    fn the_format_text_shortens_to_fit() {
        let t = Track {
            samplerate: "44.1 kHz".into(),
            bitdepth: "16 bit".into(),
            bitrate: "320 kbps".into(),
            ..Default::default()
        };
        assert_eq!(format_text(&t, 200), "44.1k 16b");
        assert_eq!(format_text(&t, 40), "44.1k");
        let radio = Track {
            bitrate: "128 kbps".into(),
            ..Default::default()
        };
        assert_eq!(format_text(&radio, 200), "128k");
    }

    #[test]
    fn the_standby_page_shows_time_address_and_date() {
        let net = Net {
            connection: Connection::Ethernet,
            ip: "10.0.0.5".into(),
        };
        let mut c = Canvas::new(Format::Grey4, 256, 64);
        standby(&mut c, (9, 5), "Thu 16 Oct 2026", &net);
        let rows = lit_rows(&c);
        assert!(rows.iter().any(|&y| y < 12), "address row");
        assert!(rows.iter().any(|&y| y > 50), "date row");
        let mut c = Canvas::new(Format::Paged1, 128, 32);
        standby(&mut c, (9, 5), "Thu 16 Oct 2026", &net);
        assert!(!lit_rows(&c).is_empty());
        let (h, m) = local_time();
        assert!(h < 24 && m < 60);
        assert!(local_date().len() >= 11);
    }
}
