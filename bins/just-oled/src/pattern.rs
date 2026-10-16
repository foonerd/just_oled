//! The test picture: what `--test` and the settings page's Test button
//! draw, made so one look tells whether the window, the orientation, the
//! colour order and the pixel packing are right.
//!
//! Top-left: a filled square (which way is up). Border: one pixel all
//! round (the window reaches every edge, no offset). Diagonal from the
//! top-left corner. Three bars, red, green, blue, left to right (colour
//! order; on a grey or one-bit panel they come out as three greys or
//! three blocks). Under them a ramp from black to white (packing).

use justoled::canvas::{Canvas, Colour};
use justoled::catalog::Format;

pub fn draw(canvas: &mut Canvas) {
    let w = canvas.width() as i32;
    let h = canvas.height() as i32;
    canvas.clear();
    canvas.rect(0, 0, w, h, Colour::WHITE);

    // Orientation marker, with a gap from the border.
    let mark = (w.min(h) / 8).max(3);
    canvas.fill_rect(2, 2, mark, mark, Colour::WHITE);

    // Diagonal, inside the border.
    let steps = w.max(h) - 2;
    for i in 0..steps {
        let x = 1 + i * (w - 2) / steps;
        let y = 1 + i * (h - 2) / steps;
        canvas.set(x, y, Colour::WHITE);
    }

    // Colour bars across the middle band.
    let band_top = h / 3;
    let band_h = (h / 4).max(2);
    let third = (w - 4) / 3;
    let bars = [
        Colour::rgb(255, 0, 0),
        Colour::rgb(0, 255, 0),
        Colour::rgb(0, 0, 255),
    ];
    for (i, colour) in bars.iter().enumerate() {
        let x = 2 + i as i32 * third;
        let bar = match canvas.format() {
            // Three greys a one-bit panel cannot show become three blocks with gaps.
            Format::Paged1 => Colour::WHITE,
            Format::Grey4 => Colour::grey([255, 170, 85][i]),
            Format::Rgb565 => *colour,
        };
        let gap = if canvas.format() == Format::Paged1 {
            2
        } else {
            0
        };
        canvas.fill_rect(x, band_top, third - gap, band_h, bar);
    }

    // Ramp from black to white along the lower band.
    let ramp_top = band_top + band_h + 2;
    let ramp_h = (h - ramp_top - 2).max(1);
    for x in 2..w - 2 {
        let level = ((x - 2) * 255 / (w - 5).max(1)) as u8;
        canvas.vline(x, ramp_top, ramp_h, Colour::grey(level));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_picture_has_its_marks_on_every_format() {
        for (format, w, h) in [
            (Format::Rgb565, 128u16, 128u16),
            (Format::Grey4, 256, 64),
            (Format::Paged1, 128, 32),
        ] {
            let mut c = Canvas::new(format, w, h);
            draw(&mut c);
            let (w, h) = (w as i32, h as i32);
            // Border corners lit, inside the marker lit, the far corner inside the border dark.
            assert_eq!(c.get(0, 0), Colour::WHITE, "{format:?}");
            assert_eq!(c.get(w - 1, h - 1), Colour::WHITE, "{format:?}");
            assert_eq!(c.get(3, 3), Colour::WHITE, "{format:?}");
            assert_eq!(c.get(w - 3, 3), Colour::BLACK, "{format:?}");
            // The diagonal reaches the far corner.
            assert_eq!(c.get(w - 2, h - 2), Colour::WHITE, "{format:?}");
        }
        let mut c = Canvas::new(Format::Rgb565, 128, 128);
        draw(&mut c);
        assert_eq!(c.get(10, 50), Colour::rgb(255, 0, 0));
        assert_eq!(c.get(64, 50), Colour::rgb(0, 255, 0));
        assert_eq!(c.get(120, 50), Colour::rgb(0, 0, 255));
        assert_eq!(c.get(2, 100), Colour::BLACK);
        assert_eq!(c.get(125, 100), Colour::WHITE);
    }
}
