//! What the panel shows: the lifecycle texts, the now-playing page with
//! the bars, the standby clock. Laid out by the panel's height class so
//! one set of pages serves a 128x32 strip and a 128x128 square alike.
//!
//! Colour panels get colour; a grey or one-bit panel gets the same shapes
//! in its own levels, the canvas folding the colours.

use justoled::canvas::{Canvas, Colour};
use justoled::catalog::Format;
use justoled::font::{Font, LARGE, MEDIUM, SMALL};

use crate::control::Track;
use crate::meter::Bars;

/// The colours of the pages, folded by the canvas on a monochrome panel.
struct Palette {
    title: Colour,
    detail: Colour,
    bar_low: Colour,
    bar_high: Colour,
    peak: Colour,
}

fn palette(format: Format) -> Palette {
    match format {
        Format::Rgb565 => Palette {
            title: Colour::WHITE,
            detail: Colour::rgb(160, 160, 170),
            bar_low: Colour::rgb(0, 200, 120),
            bar_high: Colour::rgb(255, 180, 0),
            peak: Colour::rgb(255, 80, 60),
        },
        _ => Palette {
            title: Colour::WHITE,
            detail: Colour::grey(200),
            bar_low: Colour::WHITE,
            bar_high: Colour::WHITE,
            peak: Colour::WHITE,
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
    let from = ladder
        .iter()
        .position(|f| std::ptr::eq(*f, start))
        .unwrap_or(0);
    for font in &ladder[from..] {
        if rows.clone().all(|r| font.width(r) <= width) {
            return font;
        }
    }
    SMALL
}

/// How many bars a panel this wide shows: three pixels and a gap each.
pub fn bar_count(width: u16) -> usize {
    ((width as usize + 1) / 4).clamp(4, 64)
}

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

/// The now-playing page: the title, the artist where there is room, the
/// bars below. With `blind`, the bars' place says the tap is missing.
pub fn playing(canvas: &mut Canvas, track: &Track, bars: &Bars, blind: bool) {
    canvas.clear();
    let w = canvas.width() as i32;
    let h = canvas.height() as i32;
    let pal = palette(canvas.format());
    let (title_font, detail_font, rows) = match h {
        0..=39 => (SMALL, SMALL, 1),
        // The large face fits five or six letters across 128 pixels: titles stay medium.
        _ => (MEDIUM, SMALL, 2),
    };
    let title = if track.title.is_empty() {
        "Playing"
    } else {
        &track.title
    };
    let mut y = 0;
    let fitted = title_font.fit(title, w - 2);
    canvas.text(title_font, 1, y, &fitted, pal.title);
    y += title_font.line_height as i32;
    if rows > 1 {
        let detail = if track.artist.is_empty() {
            &track.album
        } else {
            &track.artist
        };
        let fitted = detail_font.fit(detail, w - 2);
        canvas.text(detail_font, 1, y, &fitted, pal.detail);
        y += detail_font.line_height as i32;
    }
    let area_top = y + 1;
    let area_h = h - area_top;
    if area_h < 4 {
        return;
    }
    if blind {
        let note = "no audio tap";
        let font = font_that_fits(area_h as u16 * 2, w - 2, std::iter::once(note));
        let fitted = font.fit(note, w - 2);
        canvas.text_centred(
            font,
            0,
            area_top + (area_h - font.line_height as i32).max(0) / 2,
            w,
            &fitted,
            pal.detail,
        );
        return;
    }
    // One-bit audio has no bank: the level alone is drawn, as one wide bar.
    if bars.levels().iter().all(|&l| l == 0.0) && bars.peak_level() > 0.0 {
        draw_level(canvas, 0, area_top, w, area_h, bars, &pal);
        return;
    }
    draw_bars(canvas, 0, area_top, w, area_h, bars, &pal);
}

/// The RMS as a bar across the box and the peak as a mark, in decibels.
fn draw_level(canvas: &mut Canvas, x: i32, y: i32, w: i32, h: i32, bars: &Bars, pal: &Palette) {
    let bar_h = (h / 3).clamp(2, 12);
    let top = y + (h - bar_h) / 2;
    let rms = (crate::meter::height(bars.rms_level()) * w as f32).round() as i32;
    let peak = (crate::meter::height(bars.peak_level()) * w as f32).round() as i32;
    canvas.fill_rect(x, top, rms, bar_h, pal.bar_low);
    if peak > rms {
        canvas.fill_rect(x + peak - 1, top, 1, bar_h, pal.peak);
    }
}

/// The bars in a box: each `levels[i]` a column from the bottom, the peak
/// mark a line above it, a one-pixel gap between bars.
fn draw_bars(canvas: &mut Canvas, x: i32, y: i32, w: i32, h: i32, bars: &Bars, pal: &Palette) {
    let n = bars.count() as i32;
    if n == 0 || w < n {
        return;
    }
    let pitch = w / n;
    let bar_w = (pitch - 1).max(1);
    let left = x + (w - pitch * n) / 2;
    let colour_split = h * 7 / 10;
    for (i, (&level, &peak)) in bars.levels().iter().zip(bars.peaks()).enumerate() {
        let bx = left + i as i32 * pitch;
        let lit = (level * h as f32).round() as i32;
        if lit > 0 {
            let low = lit.min(colour_split);
            canvas.fill_rect(bx, y + h - low, bar_w, low, pal.bar_low);
            if lit > colour_split {
                canvas.fill_rect(bx, y + h - lit, bar_w, lit - colour_split, pal.bar_high);
            }
        }
        let mark = (peak * h as f32).round() as i32;
        if mark > 0 && mark > lit {
            canvas.fill_rect(bx, y + h - mark, bar_w, 1, pal.peak);
        }
    }
}

/// The standby page: the time, as large as the panel allows, centred.
pub fn standby(canvas: &mut Canvas, hour: u32, minute: u32) {
    canvas.clear();
    let w = canvas.width() as i32;
    let h = canvas.height() as i32;
    let text = format!("{hour:02}:{minute:02}");
    let pal = palette(canvas.format());
    // The face and its pixel doubling by what fits: the large face is 30 rows, doubled 60.
    let (font, scale) = match h {
        0..=39 => (MEDIUM, 1),
        40..=79 => (LARGE, 1),
        _ => (LARGE, 2),
    };
    let text_w = font.width(&text) * scale;
    let text_h = font.line_height as i32 * scale;
    canvas.text_scaled(
        font,
        (w - text_w) / 2,
        (h - text_h) / 2,
        &text,
        pal.title,
        scale,
    );
}

/// The local hour and minute, through the C library's time zone handling.
pub fn local_time() -> (u32, u32) {
    let now = unsafe { libc::time(std::ptr::null_mut()) };
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe {
        libc::localtime_r(&now, &mut tm);
    }
    (tm.tm_hour as u32, tm.tm_min as u32)
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

    #[test]
    fn fonts_and_bar_counts_follow_the_panel() {
        assert_eq!(font_for(32).line_height, 15);
        assert_eq!(font_for(64).line_height, 20);
        assert_eq!(font_for(128).line_height, 30);
        assert_eq!(bar_count(128), 32);
        assert_eq!(bar_count(256), 64);
        assert_eq!(bar_count(96), 24);
        assert_eq!(bar_count(64), 16);
    }

    #[test]
    fn text_rows_are_centred_and_cut_to_what_fits() {
        let mut c = Canvas::new(Format::Paged1, 128, 32);
        lines(&mut c, &["Volumio".into(), "booting...".into()]);
        let rows = lit_rows(&c);
        assert!(!rows.is_empty());
        assert!(*rows.first().unwrap() >= 1 && *rows.last().unwrap() <= 30);
        // Three rows on a 32-pixel panel: only two drawn, nothing panics.
        lines(&mut c, &["a".into(), "b".into(), "c".into()]);
        let mut c = Canvas::new(Format::Rgb565, 128, 128);
        lines(&mut c, &["Volumio".into(), "ready".into()]);
        let rows = lit_rows(&c);
        let (top, bottom) = (*rows.first().unwrap(), *rows.last().unwrap());
        assert!((top - (127 - bottom)).abs() <= 8, "{top} {bottom}");
    }

    #[test]
    fn the_playing_page_has_text_above_and_bars_below() {
        let mut bars = Bars::new(bar_count(128));
        let frame = justoled::ring::Frame {
            spectrum: [vec![1.0; 64], vec![1.0; 64]],
            hold: [vec![1.0; 64], vec![1.0; 64]],
            ..Default::default()
        };
        bars.feed(&frame, std::time::Duration::from_millis(50));
        let track = Track {
            title: "A Song".into(),
            artist: "Someone".into(),
            ..Default::default()
        };
        let mut c = Canvas::new(Format::Rgb565, 128, 128);
        playing(&mut c, &track, &bars, false);
        // Full bars reach the bottom row and are coloured (the low colour, quantised).
        let bottom = c.get(1, 127);
        assert!(bottom.g > 150 && bottom.r == 0, "{bottom:?}");
        // The gap column between bars stays dark.
        assert_eq!(c.get(3, 127), Colour::BLACK);
        // Title ink in the first rows.
        assert!(lit_rows(&c).iter().any(|&y| y < 30));

        let mut c = Canvas::new(Format::Paged1, 128, 32);
        playing(&mut c, &track, &bars, true);
        assert!(
            lit_rows(&c).iter().any(|&y| y > 16),
            "the blind note is drawn"
        );
        let mut c = Canvas::new(Format::Paged1, 128, 32);
        playing(&mut c, &track, &Bars::new(32), false);
        assert!(
            lit_rows(&c).iter().all(|&y| y < 16),
            "silent bars draw nothing"
        );

        // One-bit audio: a level bar across the box instead of the spectrum.
        let mut dsd = Bars::new(32);
        dsd.feed(
            &justoled::ring::Frame {
                one_bit: true,
                peak: [0.5, 0.5],
                rms: [0.1, 0.1],
                ..Default::default()
            },
            std::time::Duration::from_millis(50),
        );
        let mut c = Canvas::new(Format::Paged1, 128, 32);
        playing(&mut c, &track, &dsd, false);
        assert!(
            lit_rows(&c).iter().any(|&y| y > 16),
            "the level bar is drawn"
        );
    }

    #[test]
    fn the_standby_page_shows_a_time() {
        let mut c = Canvas::new(Format::Grey4, 256, 64);
        standby(&mut c, 9, 5);
        assert!(!lit_rows(&c).is_empty());
        let (h, m) = local_time();
        assert!(h < 24 && m < 60);
    }
}
