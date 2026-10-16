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

/// The colours a colour panel draws with: the words, the bars, the peak
/// mark and the progress line. A grey or one-bit panel keeps its own
/// levels whatever the theme.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Theme {
    pub title: Colour,
    pub detail: Colour,
    pub bar_low: Colour,
    pub bar_high: Colour,
    pub peak: Colour,
    pub progress: Colour,
}

/// The themes offered by name; `custom` takes its colours from the store.
pub const THEMES: &[&str] = &["classic", "amber", "ice", "fire", "white", "custom"];

impl Theme {
    /// A named theme; anything unknown is the classic one.
    pub fn preset(name: &str) -> Theme {
        match name {
            "amber" => Theme {
                title: Colour::rgb(255, 190, 40),
                detail: Colour::rgb(190, 130, 20),
                bar_low: Colour::rgb(255, 150, 0),
                bar_high: Colour::rgb(255, 215, 90),
                peak: Colour::rgb(255, 245, 200),
                progress: Colour::rgb(210, 120, 0),
            },
            "ice" => Theme {
                title: Colour::WHITE,
                detail: Colour::rgb(150, 180, 210),
                bar_low: Colour::rgb(0, 140, 255),
                bar_high: Colour::rgb(130, 225, 255),
                peak: Colour::WHITE,
                progress: Colour::rgb(0, 110, 200),
            },
            "fire" => Theme {
                title: Colour::WHITE,
                detail: Colour::rgb(210, 150, 130),
                bar_low: Colour::rgb(220, 40, 0),
                bar_high: Colour::rgb(255, 170, 0),
                peak: Colour::rgb(255, 240, 150),
                progress: Colour::rgb(200, 60, 0),
            },
            "white" => Theme {
                title: Colour::WHITE,
                detail: Colour::grey(200),
                bar_low: Colour::WHITE,
                bar_high: Colour::WHITE,
                peak: Colour::WHITE,
                progress: Colour::WHITE,
            },
            _ => Theme {
                title: Colour::WHITE,
                detail: Colour::rgb(160, 160, 170),
                bar_low: Colour::rgb(0, 200, 120),
                bar_high: Colour::rgb(255, 180, 0),
                peak: Colour::rgb(255, 80, 60),
                progress: Colour::rgb(0, 150, 170),
            },
        }
    }

    /// `#rrggbb` or `rrggbb`, as the settings page's colour picker writes it.
    pub fn parse_colour(text: &str) -> Option<Colour> {
        let hex = text.trim().trim_start_matches('#');
        if hex.len() != 6 {
            return None;
        }
        let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
        Some(Colour::rgb(byte(0)?, byte(2)?, byte(4)?))
    }
}

impl Default for Theme {
    fn default() -> Self {
        Theme::preset("classic")
    }
}

fn palette(format: Format, theme: &Theme) -> Palette {
    match format {
        Format::Rgb565 => Palette {
            title: theme.title,
            detail: theme.detail,
            bar_low: theme.bar_low,
            bar_high: theme.bar_high,
            peak: theme.peak,
            progress: theme.progress,
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
/// Every part is optional: a face decides which are there, the panel's
/// size decides where.
#[derive(Clone, Copy, Debug, Default)]
pub struct Layout {
    /// The meter's box; `None` on a text face.
    pub meter: Option<Rect>,
    /// The format, volume and connection row.
    pub status: Option<Rect>,
    /// Elapsed and total time.
    pub time: Option<Rect>,
    pub album: Option<(Rect, &'static Font)>,
    pub artist: Option<(Rect, &'static Font)>,
    /// The artist and the album on one row.
    pub line: Option<(Rect, &'static Font)>,
    pub title: Option<(Rect, &'static Font)>,
    pub progress: Option<Rect>,
}

const STATUS_H: i32 = 15;
/// The volume wedge, the connection and their gaps.
const ICONS_W: i32 = 30;
/// A meter lower than this is not worth stacking the text under; the
/// info rows go beside it instead.
const MIN_METER_H: i32 = 32;

/// The face: what the panel shows while music plays. The meter kinds
/// share one arrangement of the text; the text face gives the rows the
/// meter's room; the meter face gives the meter the whole panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Face {
    /// The spectrum, both channels folded into one row of bars, with the track.
    #[default]
    Spectrum,
    /// The spectrum per channel, the left rising and the right hanging, with the track.
    SpectrumStereo,
    /// The folded spectrum with the two level bars beside it, with the track.
    SpectrumVu,
    /// Left and right level bars, with the track.
    Vu,
    /// The track alone, in larger type: no meter.
    Text,
    /// The spectrum alone, the whole panel, a progress line under it.
    MeterOnly,
}

impl Face {
    pub fn from_setting(value: &str) -> Face {
        match value {
            "spectrum_stereo" => Face::SpectrumStereo,
            "spectrum_vu" => Face::SpectrumVu,
            "vu" => Face::Vu,
            "text" => Face::Text,
            "meter" => Face::MeterOnly,
            _ => Face::Spectrum,
        }
    }

    fn has_meter(self) -> bool {
        self != Face::Text
    }
}

/// How much of the track the text rows carry under a meter.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum TrackText {
    /// The title in larger type where the panel allows, the artist and the
    /// album on rows of their own.
    Full,
    /// The title, then the artist and the album on one row.
    #[default]
    Compact,
    /// The title alone.
    Title,
}

impl TrackText {
    pub fn from_setting(value: &str) -> TrackText {
        match value {
            "full" => TrackText::Full,
            "title" => TrackText::Title,
            _ => TrackText::Compact,
        }
    }
}

/// The artist and the album as one row.
pub fn artist_album(track: &Track) -> String {
    match (track.artist.trim(), track.album.trim()) {
        ("", album) => album.to_string(),
        (artist, "") => artist.to_string(),
        (artist, album) => format!("{artist} \u{B7} {album}"),
    }
}

fn row(x: i32, y: i32, w: i32, font: &'static Font) -> (Rect, &'static Font) {
    (
        Rect {
            x,
            y,
            w,
            h: font.line_height as i32,
        },
        font,
    )
}

/// The playing page's layout for a `face` on a panel `w` by `h`, as the
/// `look` wants it.
///
/// With a meter, the rows the look turns on are laid out and the meter
/// takes what is left: the info row(s) (format, time, volume and
/// connection) pack into as few rows as the width allows, or into the
/// title row's end when only the icons remain; the text rows follow the
/// track-text choice. The meter spans the panel over the rows when that
/// leaves it `MIN_METER_H`; otherwise it takes the left half with the
/// info rows beside it, as mpd_oled lays a 64-row panel. A strip (under
/// 40 rows) puts the title over the meter. The text face stacks the rows
/// in larger type. Every face ends in a progress line.
pub fn layout(w: i32, h: i32, face: Face, look: &Look) -> Layout {
    let small = SMALL.line_height as i32;
    let progress_h = if h < 96 { 1 } else { 2 };
    let progress = Some(Rect {
        x: 0,
        y: h - progress_h,
        w,
        h: progress_h,
    });
    let body_h = h - progress_h - 1;

    if face == Face::MeterOnly {
        return Layout {
            meter: Some(Rect {
                x: 0,
                y: 0,
                w,
                h: body_h,
            }),
            progress,
            ..Layout::default()
        };
    }

    if !face.has_meter() {
        // Text: title first and largest, then what fits beneath.
        let mut lay = Layout {
            progress,
            ..Layout::default()
        };
        let mut y = 0;
        if h < 40 {
            lay.title = Some(row(0, y, w, SMALL));
            y += small;
            if y + small <= body_h {
                lay.artist = Some(row(0, y, w, SMALL));
            }
            return lay;
        }
        let title_font = if h >= 96 { LARGE } else { MEDIUM };
        lay.title = Some(row(0, y, w, title_font));
        y += title_font.line_height as i32;
        let detail_font = if h >= 96 { MEDIUM } else { SMALL };
        lay.artist = Some(row(0, y, w, detail_font));
        y += detail_font.line_height as i32;
        if y + small + STATUS_H <= body_h {
            lay.album = Some(row(0, y, w, SMALL));
            y += small;
        }
        // The last row: the time on the left, the format and icons on the right.
        let status_y = body_h - STATUS_H;
        if status_y >= y {
            let time_w = if w >= 200 { w / 3 } else { w / 2 };
            lay.time = Some(Rect {
                x: 0,
                y: status_y,
                w: time_w,
                h: STATUS_H,
            });
            lay.status = Some(Rect {
                x: time_w,
                y: status_y,
                w: w - time_w,
                h: STATUS_H,
            });
        }
        return lay;
    }

    if h < 40 {
        return Layout {
            meter: Some(Rect {
                x: 0,
                y: small,
                w,
                h: body_h - small,
            }),
            title: Some(row(0, 0, w, SMALL)),
            progress,
            ..Layout::default()
        };
    }

    // The text rows: the title at the foot, as mpd_oled, what else the
    // choice asks for above it.
    let full = look.text == TrackText::Full;
    let mut title_font = if full && h >= 120 { MEDIUM } else { SMALL };
    let mut with_album = full && h >= 120;
    let mut with_artist = full;
    let mut with_line = look.text == TrackText::Compact;

    // The info items and the rows they take across the panel: one when
    // they fit, two otherwise (format with the icons, then the time),
    // none when only the icons are left, which ride in the title row.
    let mut format_on = look.show_format;
    let time_on = look.time != TimeDisplay::Off;
    let time_w = match look.time {
        TimeDisplay::Off => 0,
        TimeDisplay::ElapsedTotal => SMALL.width("00:00 / 00:00") + 3,
        _ => SMALL.width("-00:00") + 3,
    };
    let format_width = |on: bool| if on { SMALL.width("44.1k 16b") + 3 } else { 0 };
    let rows_for = |format_on: bool| -> i32 {
        if !format_on && !time_on {
            0
        } else if format_width(format_on) + time_w + ICONS_W <= w {
            1
        } else {
            1 + time_on as i32
        }
    };
    let mut gap = 3;
    let stacked_h = |title_font: &Font, extra_rows: i32, info_rows: i32, gap: i32| {
        body_h
            - title_font.line_height as i32
            - extra_rows * small
            - info_rows * small
            - gap * (1 + (info_rows > 0) as i32)
    };
    let extra = |a: bool, b: bool, c: bool| [a, b, c].iter().filter(|&&on| on).count() as i32;

    // Across the panel: the meter keeps at least the upper half, the rows
    // give way from the top (album, artist, the artist · album row, then
    // the format's row, the larger title, the gaps) until the rest fits.
    if look.wide_meter {
        let floor = body_h / 2;
        loop {
            let left = stacked_h(
                title_font,
                extra(with_album, with_artist, with_line),
                rows_for(format_on),
                gap,
            );
            if left >= floor {
                break;
            }
            if with_album {
                with_album = false;
            } else if with_artist {
                with_artist = false;
            } else if with_line {
                with_line = false;
            } else if format_on && rows_for(format_on) > 1 {
                format_on = false;
            } else if title_font.line_height != SMALL.line_height {
                title_font = SMALL;
            } else if gap > 0 {
                gap = 0;
            } else {
                break;
            }
        }
    }

    let format_w = format_width(format_on);
    let info_rows = rows_for(format_on);
    let text_h = title_font.line_height as i32 + extra(with_album, with_artist, with_line) * small;
    let stacked_meter_h = stacked_h(
        title_font,
        extra(with_album, with_artist, with_line),
        info_rows,
        gap,
    );

    let mut lay = Layout {
        progress,
        ..Layout::default()
    };
    let text_top = if look.wide_meter || stacked_meter_h >= MIN_METER_H || w < 96 {
        // Stacked: the meter across the panel, the info rows, the text.
        let meter_h = stacked_meter_h.max(8);
        lay.meter = Some(Rect {
            x: 0,
            y: 0,
            w,
            h: meter_h,
        });
        let mut y = meter_h + gap;
        match info_rows {
            0 => {}
            1 => {
                // The time on the left with the room the format and icons leave.
                let tw = if time_on { w - format_w - ICONS_W } else { 0 };
                if time_on {
                    lay.time = Some(Rect {
                        x: 0,
                        y,
                        w: tw,
                        h: small,
                    });
                }
                lay.status = Some(Rect {
                    x: tw,
                    y,
                    w: w - tw,
                    h: small,
                });
                y += small + gap;
            }
            _ => {
                lay.status = Some(Rect {
                    x: 0,
                    y,
                    w,
                    h: small,
                });
                y += small;
                lay.time = Some(Rect {
                    x: 0,
                    y,
                    w,
                    h: small,
                });
                y += small + gap;
            }
        }
        y
    } else {
        // Two columns: the meter left, the info rows right, the text
        // beneath. The column is as wide as its rows need, up to half.
        let col_w = (format_w + ICONS_W).max(time_w).max(ICONS_W) + 4;
        let col_w = col_w.min(w / 2 - 2);
        let col_x = w - col_w;
        let top_h = body_h - text_h;
        lay.meter = Some(Rect {
            x: 0,
            y: 0,
            w: col_x - 4,
            h: top_h,
        });
        lay.status = Some(Rect {
            x: col_x,
            y: 0,
            w: col_w,
            h: STATUS_H,
        });
        if time_on {
            lay.time = Some(Rect {
                x: col_x,
                y: STATUS_H + 1,
                w: col_w,
                h: (top_h - STATUS_H - 1).max(small),
            });
        }
        top_h
    };

    let mut y = text_top;
    if with_album {
        lay.album = Some(row(0, y, w, SMALL));
        y += small;
    }
    if with_artist {
        lay.artist = Some(row(0, y, w, SMALL));
        y += small;
    }
    if with_line {
        lay.line = Some(row(0, y, w, SMALL));
        y += small;
    }
    let mut title = row(0, y, w, title_font);
    if lay.status.is_none() {
        // Only the icons: the end of the title row is theirs.
        lay.status = Some(Rect {
            x: w - ICONS_W,
            y: title.0.y,
            w: ICONS_W,
            h: title.0.h,
        });
        title.0.w = w - ICONS_W - 2;
    }
    lay.title = Some(title);
    lay
}

/// How many bars the spectrum shows on a panel: three pixels and a gap
/// each across the meter's box, for the look chosen.
pub fn bar_count(width: u16, height: u16, look: &Look) -> usize {
    let meter = layout(width as i32, height as i32, look.face, look)
        .meter
        .unwrap_or(Rect {
            x: 0,
            y: 0,
            w: width as i32,
            h: 0,
        });
    let w = match look.face {
        // The level bars take a third beside the spectrum on a wide box.
        Face::SpectrumVu if meter.w >= 2 * meter.h => meter.w - meter.w / 3 - 3,
        _ => meter.w,
    };
    ((w as usize + 1) / 4).clamp(4, 64)
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
pub fn lines(canvas: &mut Canvas, rows: &[String], theme: &Theme) {
    canvas.clear();
    let w = canvas.width() as i32;
    let h = canvas.height() as i32;
    let font = font_that_fits(canvas.height(), w - 2, rows.iter().map(String::as_str));
    let line = font.line_height as i32;
    let shown = rows.len().min((h / line).max(1) as usize);
    let block = shown as i32 * line;
    let top = (h - block) / 2;
    let pal = palette(canvas.format(), theme);
    for (i, row) in rows.iter().take(shown).enumerate() {
        let colour = if i == 0 { pal.title } else { pal.detail };
        let fitted = font.fit(row, w - 2);
        canvas.text_centred(font, 0, top + i as i32 * line, w, &fitted, colour);
    }
}

/// The time row of the playing page.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum TimeDisplay {
    #[default]
    ElapsedTotal,
    Elapsed,
    Remaining,
    Off,
}

impl TimeDisplay {
    pub fn from_setting(value: &str) -> TimeDisplay {
        match value {
            "elapsed" => TimeDisplay::Elapsed,
            "remaining" => TimeDisplay::Remaining,
            "off" => TimeDisplay::Off,
            _ => TimeDisplay::ElapsedTotal,
        }
    }
}

/// The user's choices for the pages.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Look {
    pub face: Face,
    pub text: TrackText,
    pub time: TimeDisplay,
    /// The sample rate and bit depth in the status row.
    pub show_format: bool,
    /// Peak marks on the meters.
    pub peaks: bool,
    /// Decibels added to the meters, for quiet or loud masters.
    pub gain_db: i32,
    pub show_ip: bool,
    /// The colours on a colour panel.
    pub theme: Theme,
    /// The meter across the panel's width and at least its upper half,
    /// the rows making do with the rest.
    pub wide_meter: bool,
}

impl Default for Look {
    fn default() -> Self {
        Look {
            face: Face::Spectrum,
            text: TrackText::Compact,
            time: TimeDisplay::ElapsedTotal,
            show_format: true,
            peaks: true,
            gain_db: 0,
            show_ip: true,
            theme: Theme::default(),
            wide_meter: false,
        }
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
    pub net: &'a Net,
    pub look: &'a Look,
    pub album: &'a Marquee,
    pub artist: &'a Marquee,
    /// The artist and the album as one row.
    pub line: &'a Marquee,
    pub title: &'a Marquee,
    pub now: Instant,
}

/// Seconds as m:ss, or h:mm:ss past the hour.
pub fn clock_text(seconds: i64) -> String {
    let s = seconds.max(0);
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, (s % 3600) / 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

/// The time row as the user chose it; empty when off.
fn time_text(look: &Look, elapsed_s: i64, duration_s: i64) -> String {
    match look.time {
        TimeDisplay::Off => String::new(),
        TimeDisplay::Elapsed => clock_text(elapsed_s),
        TimeDisplay::Remaining if duration_s > 0 => {
            format!("-{}", clock_text(duration_s - elapsed_s))
        }
        TimeDisplay::Remaining => clock_text(elapsed_s),
        TimeDisplay::ElapsedTotal if duration_s > 0 => {
            format!("{} / {}", clock_text(elapsed_s), clock_text(duration_s))
        }
        TimeDisplay::ElapsedTotal => clock_text(elapsed_s),
    }
}

/// The now-playing page.
pub fn playing(canvas: &mut Canvas, view: &View) {
    canvas.clear();
    let w = canvas.width() as i32;
    let h = canvas.height() as i32;
    let pal = palette(canvas.format(), &view.look.theme);
    let lay = layout(w, h, view.look.face, view.look);

    // The meter, or the reason there is none.
    if let Some(meter) = lay.meter {
        if view.blind {
            let note = "no audio tap";
            let font = font_that_fits(meter.h as u16 * 2, meter.w - 2, std::iter::once(note));
            let fitted = font.fit(note, meter.w - 2);
            canvas.text_centred(
                font,
                meter.x,
                meter.y + (meter.h - font.line_height as i32).max(0) / 2,
                meter.w,
                &fitted,
                pal.detail,
            );
        } else {
            draw_meter(canvas, meter, view, &pal);
        }
    }

    if let Some(status) = lay.status {
        let beside_time = lay.time.is_some_and(|t| t.y == status.y);
        draw_status(canvas, status, view, &pal, beside_time);
    }
    if let Some(rect) = lay.time {
        let text = time_text(view.look, view.elapsed_s, view.track.duration);
        let fitted = SMALL.fit(&text, rect.w - 2);
        // Centred where the row is its own; left-aligned beside the status.
        if lay.status.is_some_and(|s| s.y == rect.y) {
            canvas.text(SMALL, rect.x + 1, rect.y, &fitted, pal.detail);
        } else {
            canvas.text_centred(SMALL, rect.x, rect.y, rect.w, &fitted, pal.detail);
        }
    }
    if let Some((rect, font)) = lay.album {
        draw_marquee(canvas, font, rect, view.album, view.now, pal.detail);
    }
    if let Some((rect, font)) = lay.artist {
        draw_marquee(canvas, font, rect, view.artist, view.now, pal.detail);
    }
    if let Some((rect, font)) = lay.line {
        draw_marquee(canvas, font, rect, view.line, view.now, pal.detail);
    }
    if let Some((rect, font)) = lay.title {
        if view.title.text().is_empty() {
            canvas.text(font, rect.x + 1, rect.y, "Playing", pal.title);
        } else {
            draw_marquee(canvas, font, rect, view.title, view.now, pal.title);
        }
    }

    if let Some(progress) = lay.progress {
        if view.track.duration > 0 {
            let done = (progress.w as i64 * view.elapsed_s.clamp(0, view.track.duration)
                / view.track.duration) as i32;
            canvas.fill_rect(progress.x, progress.y, done, progress.h, pal.progress);
        }
    }
}

/// The meter the face asks for, in its box. One-bit audio has no bank,
/// so a spectrum falls back on the level.
fn draw_meter(canvas: &mut Canvas, r: Rect, view: &View, pal: &Palette) {
    let bars = view.bars;
    let one_bit = bars.levels().iter().all(|&l| l == 0.0) && bars.peak_level() > 0.0;
    let peaks = view.look.peaks;
    match view.look.face {
        Face::Text => {}
        Face::Vu => draw_vu(canvas, r, bars, peaks, pal),
        _ if one_bit => draw_level(canvas, r, bars, pal),
        Face::Spectrum | Face::MeterOnly => draw_bars(canvas, r, bars, peaks, pal),
        Face::SpectrumStereo => draw_bars_stereo(canvas, r, bars, peaks, pal),
        Face::SpectrumVu => {
            // The level bars take the right third, or the lower third on a narrow box.
            if r.w >= 2 * r.h {
                let vu_w = r.w / 3;
                draw_bars(
                    canvas,
                    Rect {
                        w: r.w - vu_w - 3,
                        ..r
                    },
                    bars,
                    peaks,
                    pal,
                );
                draw_vu(
                    canvas,
                    Rect {
                        x: r.x + r.w - vu_w,
                        w: vu_w,
                        ..r
                    },
                    bars,
                    peaks,
                    pal,
                );
            } else {
                let vu_h = (r.h / 3).max(12);
                draw_bars(
                    canvas,
                    Rect {
                        h: r.h - vu_h - 2,
                        ..r
                    },
                    bars,
                    peaks,
                    pal,
                );
                draw_vu(
                    canvas,
                    Rect {
                        y: r.y + r.h - vu_h,
                        h: vu_h,
                        ..r
                    },
                    bars,
                    peaks,
                    pal,
                );
            }
        }
    }
}

/// The spectrum per channel: the left channel's bars rise from the middle
/// line, the right channel's hang from it, the same band under each other.
fn draw_bars_stereo(canvas: &mut Canvas, r: Rect, bars: &Bars, peaks: bool, pal: &Palette) {
    let n = bars.count() as i32;
    if n == 0 || r.w < n || r.h < 6 {
        return;
    }
    let pitch = r.w / n;
    let bar_w = (pitch - 1).max(1);
    let left = r.x + (r.w - pitch * n) / 2;
    let half = (r.h - 1) / 2;
    let mid = r.y + half;
    canvas.hline(r.x, mid, r.w, pal.detail);
    for i in 0..n as usize {
        let bx = left + i as i32 * pitch;
        let split = half * 7 / 10;
        // Left: upwards from the middle.
        let lit = (bars.levels_ch(0)[i] * half as f32).round() as i32;
        if lit > 0 {
            let low = lit.min(split);
            canvas.fill_rect(bx, mid - low, bar_w, low, pal.bar_low);
            if lit > split {
                canvas.fill_rect(bx, mid - lit, bar_w, lit - split, pal.bar_high);
            }
        }
        let mark = (bars.peaks_ch(0)[i] * half as f32).round() as i32;
        if peaks && mark > lit && mark > 0 {
            canvas.fill_rect(bx, mid - mark, bar_w, 1, pal.peak);
        }
        // Right: downwards from the middle.
        let lit = (bars.levels_ch(1)[i] * half as f32).round() as i32;
        if lit > 0 {
            let low = lit.min(split);
            canvas.fill_rect(bx, mid + 1, bar_w, low, pal.bar_low);
            if lit > split {
                canvas.fill_rect(bx, mid + 1 + split, bar_w, lit - split, pal.bar_high);
            }
        }
        let mark = (bars.peaks_ch(1)[i] * half as f32).round() as i32;
        if peaks && mark > lit && mark > 0 {
            canvas.fill_rect(bx, mid + mark, bar_w, 1, pal.peak);
        }
    }
}

/// The stream's format on the left, the volume and the connection on the
/// right; beside the time, the format keeps to the icons instead.
fn draw_status(canvas: &mut Canvas, rect: Rect, view: &View, pal: &Palette, beside_time: bool) {
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
    let text = if view.look.show_format {
        format_text(view.track, room)
    } else {
        String::new()
    };
    let x = if beside_time {
        right - 5 - SMALL.width(&text)
    } else {
        rect.x
    };
    canvas.set_clip(rect.x, rect.y, room, rect.h);
    canvas.text(SMALL, x.max(rect.x), rect.y, &text, pal.detail);
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
    let rms = (bars.height(bars.rms_level()) * r.w as f32).round() as i32;
    let peak = (bars.height(bars.peak_level()) * r.w as f32).round() as i32;
    canvas.fill_rect(r.x, top, rms, bar_h, pal.bar_low);
    if peak > rms {
        canvas.fill_rect(r.x + peak - 1, top, 1, bar_h, pal.peak);
    }
}

/// Left and right as two horizontal bars with their letters: the RMS
/// filled, the peak a mark, the top tenth in the high colour. A box too
/// low for the letters shows the two bars alone.
fn draw_vu(canvas: &mut Canvas, r: Rect, bars: &Bars, peaks: bool, pal: &Palette) {
    let row_h = r.h / 2;
    let lettered = row_h >= SMALL.line_height as i32 - 3;
    let label_w = if lettered { SMALL.width("R") + 3 } else { 0 };
    let bar_x = r.x + label_w;
    let bar_w = r.w - label_w;
    let bar_h = (row_h - if lettered { 4 } else { 2 }).clamp(2, 12);
    let rms = bars.rms_lr();
    let peak = bars.peak_lr();
    for (ch, letter) in ["L", "R"].iter().enumerate() {
        let row_y = r.y + ch as i32 * row_h;
        let top = row_y + (row_h - bar_h) / 2;
        if lettered {
            canvas.text(
                SMALL,
                r.x,
                row_y + (row_h - SMALL.line_height as i32) / 2,
                letter,
                pal.detail,
            );
        }
        let filled = (rms[ch] * bar_w as f32).round() as i32;
        let split = bar_w * 8 / 10;
        canvas.fill_rect(bar_x, top, filled.min(split), bar_h, pal.bar_low);
        if filled > split {
            canvas.fill_rect(bar_x + split, top, filled - split, bar_h, pal.bar_high);
        }
        let mark = (peak[ch] * bar_w as f32).round() as i32;
        if peaks && mark > filled && mark > 0 {
            canvas.fill_rect(bar_x + mark - 1, top, 1, bar_h, pal.peak);
        }
        // A faint scale line under each bar.
        canvas.hline(bar_x, top + bar_h, bar_w, pal.detail);
    }
}

/// The bars in a box: each `levels[i]` a column from the bottom, the peak
/// mark a line above it, a one-pixel gap between bars.
fn draw_bars(canvas: &mut Canvas, r: Rect, bars: &Bars, peaks: bool, pal: &Palette) {
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
        if peaks && mark > 0 && mark > lit {
            canvas.fill_rect(bx, r.y + r.h - mark, bar_w, 1, pal.peak);
        }
    }
}

/// The standby page: the address and the connection along the top, the
/// time as large as the panel allows, the date below; a strip shows the
/// time alone. `clock` and `date` come formatted, as the user chose.
pub fn standby(canvas: &mut Canvas, clock: &str, date: &str, net: &Net, look: &Look) {
    canvas.clear();
    let w = canvas.width() as i32;
    let h = canvas.height() as i32;
    let pal = palette(canvas.format(), &look.theme);
    if h < 40 {
        let y = (h - MEDIUM.line_height as i32) / 2;
        canvas.text_centred(MEDIUM, 0, y, w, clock, pal.title);
        return;
    }
    let small = SMALL.line_height as i32;
    if look.show_ip && !net.ip.is_empty() {
        canvas.text(SMALL, 1, 0, &net.ip, pal.detail);
    }
    if net.connection != Connection::None {
        draw_connection(canvas, w - 13, 3, net.connection, pal.detail);
    }
    // The face and its pixel doubling by what is left between the rows,
    // stepping down when the text (a 12-hour clock with its am/pm) is wide.
    let room = h - 2 * small;
    let mut choices: Vec<(&Font, i32)> = Vec::new();
    if room >= 62 {
        choices.push((LARGE, 2));
    }
    if room >= 32 {
        choices.push((LARGE, 1));
    }
    choices.push((MEDIUM, 1));
    let (font, scale) = choices
        .iter()
        .copied()
        .find(|(f, s)| f.width(clock) * s <= w - 2)
        .unwrap_or((MEDIUM, 1));
    let text_w = font.width(clock) * scale;
    let text_h = font.line_height as i32 * scale;
    canvas.text_scaled(
        font,
        (w - text_w) / 2,
        small + (room - text_h) / 2,
        clock,
        pal.title,
        scale,
    );
    let date_font = if h >= 120 && MEDIUM.width(date) <= w - 2 {
        MEDIUM
    } else {
        SMALL
    };
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

/// The clock and date formats offered, `strftime` patterns as lcd1602-vumeter
/// offers them; anything else from the store falls back to the first.
pub const CLOCK_FORMATS: &[&str] = &["%H:%M", "%H:%M:%S", "%I:%M %p", "%l:%M %p", "%I:%M:%S %p"];
pub const DATE_FORMATS: &[&str] = &[
    "%a %d %b",
    "%A %d %B",
    "%a %d %b %Y",
    "%d.%m.%Y",
    "%d/%m/%Y",
    "%m/%d/%Y",
    "%Y-%m-%d",
    "%a %d.%m.",
    "%A",
];

/// The local time through `strftime` in the C locale: English names,
/// the player's time zone. `%p` comes back lower-case, as a panel reads
/// better with "8:45 pm".
pub fn local_strftime(format: &str) -> String {
    let now = unsafe { libc::time(std::ptr::null_mut()) };
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe {
        libc::localtime_r(&now, &mut tm);
    }
    let Ok(cformat) = std::ffi::CString::new(format) else {
        return String::new();
    };
    let mut buf = [0u8; 96];
    let n = unsafe {
        libc::strftime(
            buf.as_mut_ptr() as *mut libc::c_char,
            buf.len(),
            cformat.as_ptr(),
            &tm,
        )
    };
    let text = String::from_utf8_lossy(&buf[..n]).trim().to_string();
    if format.contains("%p") {
        text.replace("AM", "am").replace("PM", "pm")
    } else {
        text
    }
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
        look: &'a Look,
        rows: &'a [Marquee; 4],
        blind: bool,
    ) -> View<'a> {
        View {
            track,
            bars,
            blind,
            elapsed_s: 83,
            net,
            look,
            album: &rows[0],
            artist: &rows[1],
            line: &rows[2],
            title: &rows[3],
            now: Instant::now(),
        }
    }

    fn look(face: Face, text: TrackText, time: TimeDisplay, show_format: bool) -> Look {
        Look {
            face,
            text,
            time,
            show_format,
            ..Look::default()
        }
    }

    #[test]
    fn fonts_and_layouts_follow_the_panel() {
        assert_eq!(font_for(32).line_height, 15);
        assert_eq!(font_for(64).line_height, 20);
        assert_eq!(font_for(128).line_height, 30);
        let dflt = Look::default();
        // The 64-row panel halves: meter left, 15 bars on a 128 wide panel,
        // the artist and album on one row under it.
        let l = layout(128, 64, Face::Spectrum, &dflt);
        assert_eq!(
            l.meter,
            Some(Rect {
                x: 0,
                y: 0,
                w: 62,
                h: 32
            })
        );
        assert!(l.status.is_some() && l.time.is_some() && l.line.is_some());
        assert!(l.artist.is_none() && l.album.is_none());
        assert_eq!(l.progress.unwrap().y, 63);
        assert_eq!(bar_count(128, 64, &dflt), 15);
        // A wide panel's info column is only as wide as its rows need.
        assert_eq!(bar_count(256, 64, &dflt), 40);
        // A strip has no status line or time; the meter takes the width.
        let l = layout(128, 32, Face::Spectrum, &dflt);
        assert!(l.status.is_none() && l.time.is_none() && l.artist.is_none());
        assert_eq!(bar_count(128, 32, &dflt), 32);
        // A tall panel stacks everything; full text grows the title and
        // gives the album its row, at the meter's cost.
        let l = layout(128, 128, Face::Spectrum, &dflt);
        assert_eq!(l.meter.unwrap().w, 128);
        let compact_h = l.meter.unwrap().h;
        assert_eq!(l.title.unwrap().1.line_height, 15);
        let full = look(
            Face::Spectrum,
            TrackText::Full,
            TimeDisplay::ElapsedTotal,
            true,
        );
        let l = layout(128, 128, Face::Spectrum, &full);
        let (title, font) = l.title.unwrap();
        assert_eq!(font.line_height, 20);
        assert!(l.album.is_some() && l.artist.is_some() && l.line.is_none());
        assert!(title.y + title.h <= l.progress.unwrap().y, "{l:?}");
        assert!(l.meter.unwrap().h < compact_h, "{l:?}");
        // Rows turned off hand their height to the meter: the time shares
        // the format's row, then the icons ride in the title row.
        let no_time = look(Face::Spectrum, TrackText::Compact, TimeDisplay::Off, true);
        let l = layout(128, 128, Face::Spectrum, &no_time);
        assert!(l.meter.unwrap().h > compact_h, "{l:?}");
        assert!(l.time.is_none() && l.status.unwrap().y < l.title.unwrap().0.y);
        let no_format = look(
            Face::Spectrum,
            TrackText::Compact,
            TimeDisplay::ElapsedTotal,
            false,
        );
        let l = layout(128, 128, Face::Spectrum, &no_format);
        let (t, s) = (l.time.unwrap(), l.status.unwrap());
        assert_eq!(t.y, s.y, "one row: time left, icons right");
        assert_eq!(t.w + s.w, 128);
        let bare = look(Face::Spectrum, TrackText::Title, TimeDisplay::Off, false);
        let l = layout(128, 128, Face::Spectrum, &bare);
        let (title, _) = l.title.unwrap();
        assert_eq!(l.status.unwrap().y, title.y, "the icons in the title row");
        assert_eq!(title.w, 128 - ICONS_W - 2);
        assert!(l.meter.unwrap().h >= 100, "{l:?}");
        // On a 64-row panel the title alone with nothing else frees the
        // whole width for the meter.
        let l = layout(128, 64, Face::Spectrum, &bare);
        assert_eq!(l.meter.unwrap().w, 128);
        assert!(l.meter.unwrap().h >= 40, "{l:?}");
        assert_eq!(bar_count(128, 64, &bare), 32);
        // Across the panel: the meter keeps the upper half whatever is on;
        // the artist · album row and the format give way, the time stays.
        let wide = Look {
            wide_meter: true,
            ..Look::default()
        };
        let l = layout(128, 64, Face::Spectrum, &wide);
        let m = l.meter.unwrap();
        assert_eq!((m.w, m.h), (128, 32), "{l:?}");
        assert!(l.line.is_none() && l.title.is_some() && l.time.is_some());
        let (t, s) = (l.time.unwrap(), l.status.unwrap());
        assert_eq!(t.y, s.y, "time and icons on one row");
        assert_eq!(s.w, ICONS_W, "no room for the format");
        assert_eq!(bar_count(128, 64, &wide), 32);
        let l = layout(256, 64, Face::Spectrum, &wide);
        assert_eq!(l.meter.unwrap().w, 256);
        assert_eq!(bar_count(256, 64, &wide), 64);
        let l = layout(128, 128, Face::Spectrum, &wide);
        assert!(l.meter.unwrap().h >= 62, "{l:?}");
        let l = layout(128, 96, Face::Spectrum, &full);
        assert!(l.meter.unwrap().h >= 20, "{l:?}");
        let (title, _) = l.title.unwrap();
        assert!(title.y + title.h <= l.progress.unwrap().y, "{l:?}");
        // The text face has no meter and a larger title; the meter face the reverse.
        let l = layout(128, 64, Face::Text, &dflt);
        assert!(l.meter.is_none());
        assert_eq!(l.title.unwrap().1.line_height, 20);
        assert!(l.time.is_some() && l.status.is_some());
        let l = layout(128, 64, Face::MeterOnly, &dflt);
        assert!(l.title.is_none() && l.status.is_none());
        assert_eq!(l.meter.unwrap().h, 62);
        let meter_only = look(
            Face::MeterOnly,
            TrackText::Compact,
            TimeDisplay::ElapsedTotal,
            true,
        );
        assert_eq!(bar_count(128, 64, &meter_only), 32);
        // The level bars beside the spectrum leave fewer bars.
        let vu = look(
            Face::SpectrumVu,
            TrackText::Compact,
            TimeDisplay::ElapsedTotal,
            true,
        );
        assert!(bar_count(256, 64, &vu) < bar_count(256, 64, &dflt));
        // The artist and album row.
        let t = Track {
            artist: "Neil Young".into(),
            album: "Harvest Moon".into(),
            ..Default::default()
        };
        assert_eq!(artist_album(&t), "Neil Young \u{B7} Harvest Moon");
        let t = Track {
            artist: "Neil Young".into(),
            ..Default::default()
        };
        assert_eq!(artist_album(&t), "Neil Young");
    }

    #[test]
    fn text_rows_are_centred_and_cut_to_what_fits() {
        let theme = Theme::default();
        let mut c = Canvas::new(Format::Paged1, 128, 32);
        lines(&mut c, &["Volumio".into(), "booting...".into()], &theme);
        let rows = lit_rows(&c);
        assert!(!rows.is_empty());
        assert!(*rows.first().unwrap() >= 1 && *rows.last().unwrap() <= 30);
        lines(&mut c, &["a".into(), "b".into(), "c".into()], &theme);
        let mut c = Canvas::new(Format::Rgb565, 128, 128);
        lines(&mut c, &["Volumio".into(), "ready".into()], &theme);
        let rows = lit_rows(&c);
        let (top, bottom) = (*rows.first().unwrap(), *rows.last().unwrap());
        assert!((top - (127 - bottom)).abs() <= 8, "{top} {bottom}");
    }

    #[test]
    fn the_theme_colours_a_colour_panel_and_leaves_a_mono_one() {
        let amber = Theme::preset("amber");
        let mut c = Canvas::new(Format::Rgb565, 128, 64);
        lines(&mut c, &["Volumio".into()], &amber);
        let lit: Vec<Colour> = (0..64)
            .flat_map(|y| (0..128).map(move |x| (x, y)))
            .map(|(x, y)| c.get(x, y))
            .filter(|&p| p != Colour::BLACK)
            .collect();
        assert!(!lit.is_empty());
        // RGB565 rounds the colour; one colour, the amber, is what matters.
        let first = lit[0];
        assert!(lit.iter().all(|&p| p == first), "all in the title colour");
        assert!(
            first.r >= 248 && (184..=192).contains(&first.g) && first.b <= 48,
            "{first:?}"
        );
        let mut c = Canvas::new(Format::Paged1, 128, 64);
        lines(&mut c, &["Volumio".into()], &amber);
        assert!(!lit_rows(&c).is_empty());
        // The picker's hex, with or without the hash; anything else is nothing.
        assert_eq!(
            Theme::parse_colour("#FF8000"),
            Some(Colour::rgb(255, 128, 0))
        );
        assert_eq!(
            Theme::parse_colour("00c878"),
            Some(Colour::rgb(0, 200, 120))
        );
        assert_eq!(Theme::parse_colour("#fff"), None);
        assert_eq!(Theme::parse_colour("red"), None);
        assert_eq!(Theme::preset("nothing"), Theme::default());
        assert!(THEMES.contains(&"custom"));
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
        let look = Look::default();
        let mut bars = Bars::new(bar_count(128, 64, &look));
        let frame = justoled::ring::Frame {
            spectrum: [vec![1.0; 64], vec![1.0; 64]],
            hold: [vec![1.0; 64], vec![1.0; 64]],
            ..Default::default()
        };
        bars.feed(&frame, Duration::from_millis(50));
        let track = Track {
            title: "A Song".into(),
            artist: "Someone".into(),
            album: "The Album".into(),
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
        let mut rows: [Marquee; 4] = Default::default();
        let now = Instant::now();
        rows[0].set(&track.album, now);
        rows[1].set(&track.artist, now);
        rows[2].set(&artist_album(&track), now);
        rows[3].set(&track.title, now);
        let mut c = Canvas::new(Format::Rgb565, 128, 64);
        playing(&mut c, &view(&track, &bars, &net, &look, &rows, false));
        // Full bars reach the bottom of their box, on the left half only.
        assert_ne!(c.get(1, 31), Colour::BLACK);
        assert_eq!(c.get(1, 32), Colour::BLACK, "under the meter box");
        // The progress line: 83 of 200 seconds is 53 of 128 pixels.
        assert_ne!(c.get(50, 63), Colour::BLACK);
        assert_eq!(c.get(60, 63), Colour::BLACK);
        // Something in the status row's right end: the icons.
        assert!((116..128).any(|x| (0..8).any(|y| c.get(x, y) != Colour::BLACK)));
        // And the time in the right half.
        assert!((66..126).any(|x| (14..32).any(|y| c.get(x, y) != Colour::BLACK)));

        let mut c = Canvas::new(Format::Paged1, 128, 32);
        playing(&mut c, &view(&track, &bars, &net, &look, &rows, true));
        assert!(
            lit_rows(&c).iter().any(|&y| y > 16),
            "the blind note is drawn"
        );

        // Every face, text choice and info-row combination draws without
        // panic on every class.
        for face in [
            Face::Spectrum,
            Face::SpectrumStereo,
            Face::SpectrumVu,
            Face::Vu,
            Face::Text,
            Face::MeterOnly,
        ] {
            for text in [TrackText::Full, TrackText::Compact, TrackText::Title] {
                for (time, show_format) in [
                    (TimeDisplay::ElapsedTotal, true),
                    (TimeDisplay::Remaining, false),
                    (TimeDisplay::Off, true),
                    (TimeDisplay::Off, false),
                ] {
                    let look = Look {
                        face,
                        text,
                        time,
                        show_format,
                        ..Look::default()
                    };
                    for (fmt, w, h) in [
                        (Format::Paged1, 128u16, 32u16),
                        (Format::Paged1, 128, 64),
                        (Format::Grey4, 256, 64),
                        (Format::Rgb565, 96, 64),
                        (Format::Rgb565, 128, 128),
                    ] {
                        let mut c = Canvas::new(fmt, w, h);
                        playing(&mut c, &view(&track, &bars, &net, &look, &rows, false));
                        assert!(!lit_rows(&c).is_empty(), "{face:?} {text:?} {w}x{h}");
                    }
                }
            }
        }

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
        playing(&mut c, &view(&track, &dsd, &net, &look, &rows, false));
        assert!(
            lit_rows(&c).iter().any(|&y| (16..30).contains(&y)),
            "the level bar is drawn"
        );
    }

    #[test]
    fn the_time_row_follows_the_choice() {
        let mut look = Look::default();
        assert_eq!(time_text(&look, 83, 200), "1:23 / 3:20");
        look.time = TimeDisplay::Elapsed;
        assert_eq!(time_text(&look, 83, 200), "1:23");
        look.time = TimeDisplay::Remaining;
        assert_eq!(time_text(&look, 83, 200), "-1:57");
        assert_eq!(time_text(&look, 83, 0), "1:23", "a stream has no end");
        look.time = TimeDisplay::Off;
        assert_eq!(time_text(&look, 83, 200), "");
        assert_eq!(clock_text(3725), "1:02:05");
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
        let look = Look::default();
        let mut c = Canvas::new(Format::Grey4, 256, 64);
        standby(&mut c, "9:05", "Thu 16 Oct 2026", &net, &look);
        let rows = lit_rows(&c);
        assert!(rows.iter().any(|&y| y < 12), "address row");
        assert!(rows.iter().any(|&y| y > 50), "date row");
        let mut c = Canvas::new(Format::Paged1, 128, 32);
        standby(&mut c, "20:45:30", "Thu 16 Oct 2026", &net, &look);
        assert!(!lit_rows(&c).is_empty());
        // Without the address, the top left stays dark; the icon keeps its corner.
        let quiet = Look {
            show_ip: false,
            ..Look::default()
        };
        let mut c = Canvas::new(Format::Grey4, 256, 64);
        standby(&mut c, "9:05", "Thu 16 Oct 2026", &net, &quiet);
        assert!(
            !(0..200).any(|x| (0..12).any(|y| c.get(x, y) != Colour::BLACK)),
            "no address row"
        );
        // strftime through the C library, in the player's zone.
        tzset_now();
        let clock = local_strftime("%H:%M");
        assert_eq!(clock.len(), 5, "{clock}");
        assert!(local_strftime("%a %d %b %Y").len() >= 11);
        let half = local_strftime("%I:%M %p");
        assert!(half.ends_with("am") || half.ends_with("pm"), "{half}");
        assert!(CLOCK_FORMATS.contains(&"%H:%M") && DATE_FORMATS.contains(&"%A"));
    }
}
