//! The frame: pixels kept in the controller's own RAM layout, so a frame is
//! sent as it is, no conversion pass. Drawing goes through one colour type
//! and the canvas folds it to its format: a bit, a grey nibble, or RGB565.
//!
//! Turning the picture upside down is a property of the canvas, applied
//! as pixels are set, so pages and the driver never know about it.

use crate::catalog::Format;
use crate::font::Font;

/// A colour as drawn. Monochrome formats take its luminance.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Colour {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Colour {
    pub const BLACK: Colour = Colour::rgb(0, 0, 0);
    pub const WHITE: Colour = Colour::rgb(255, 255, 255);

    pub const fn rgb(r: u8, g: u8, b: u8) -> Colour {
        Colour { r, g, b }
    }

    /// A grey level, the same in all three channels.
    pub const fn grey(level: u8) -> Colour {
        Colour::rgb(level, level, level)
    }

    /// Luminance 0..=255 by the Rec. 601 weights, in integer arithmetic.
    pub fn luma(self) -> u8 {
        ((self.r as u32 * 77 + self.g as u32 * 151 + self.b as u32 * 28) >> 8) as u8
    }

    /// Red 5, green 6, blue 5 in one word.
    pub fn rgb565(self) -> u16 {
        ((self.r as u16 & 0xF8) << 8) | ((self.g as u16 & 0xFC) << 3) | (self.b as u16 >> 3)
    }

    /// The top four bits of the luminance.
    pub fn grey4(self) -> u8 {
        self.luma() >> 4
    }

    /// Lit or not, for a one-bit panel.
    pub fn on(self) -> bool {
        self.luma() >= 128
    }
}

/// How the picture sits on the panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Rotate {
    #[default]
    Normal,
    /// Turned half way round, for a module mounted upside down.
    Half,
}

impl Rotate {
    /// From the settings value, anything unknown being normal.
    pub fn from_degrees(degrees: i64) -> Rotate {
        if degrees == 180 {
            Rotate::Half
        } else {
            Rotate::Normal
        }
    }
}

/// A frame of pixels in a controller's RAM layout.
#[derive(Clone, Debug)]
pub struct Canvas {
    format: Format,
    width: u16,
    height: u16,
    rotate: Rotate,
    bytes: Vec<u8>,
    /// Pixels outside this box (x0, y0, x1, y1; x1 and y1 exclusive) are not set.
    clip: Option<(i32, i32, i32, i32)>,
}

impl Canvas {
    pub fn new(format: Format, width: u16, height: u16) -> Canvas {
        Canvas {
            format,
            width,
            height,
            rotate: Rotate::Normal,
            bytes: vec![0; format.frame_bytes(width, height)],
            clip: None,
        }
    }

    pub fn with_rotate(mut self, rotate: Rotate) -> Canvas {
        self.rotate = rotate;
        self
    }

    /// Confine drawing to a box until `clear_clip`; `w` and `h` of zero or less clip everything.
    pub fn set_clip(&mut self, x: i32, y: i32, w: i32, h: i32) {
        self.clip = Some((x, y, x + w, y + h));
    }

    pub fn clear_clip(&mut self) {
        self.clip = None;
    }

    pub fn format(&self) -> Format {
        self.format
    }

    pub fn width(&self) -> u16 {
        self.width
    }

    pub fn height(&self) -> u16 {
        self.height
    }

    pub fn rotate(&self) -> Rotate {
        self.rotate
    }

    /// The frame as the controller takes it.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// One page of a one-bit frame: `width` bytes, eight rows.
    /// Other formats have no pages and get an empty slice.
    pub fn page(&self, index: usize) -> &[u8] {
        if self.format != Format::Paged1 {
            return &[];
        }
        let w = self.width as usize;
        let start = index * w;
        if start >= self.bytes.len() {
            return &[];
        }
        &self.bytes[start..start + w]
    }

    /// Pages in a one-bit frame, zero for other formats.
    pub fn pages(&self) -> usize {
        match self.format {
            Format::Paged1 => (self.height as usize).div_ceil(8),
            _ => 0,
        }
    }

    /// Everything off.
    pub fn clear(&mut self) {
        self.bytes.fill(0);
    }

    /// Everything one colour.
    pub fn fill(&mut self, colour: Colour) {
        match self.format {
            Format::Paged1 => self.bytes.fill(if colour.on() { 0xFF } else { 0 }),
            Format::Grey4 => {
                let g = colour.grey4();
                self.bytes.fill(g << 4 | g);
            }
            Format::Rgb565 => {
                let pair = colour.rgb565().to_be_bytes();
                for word in self.bytes.as_chunks_mut::<2>().0 {
                    *word = pair;
                }
            }
        }
    }

    /// Set one pixel; off the canvas, or outside the clip box, is quietly ignored.
    pub fn set(&mut self, x: i32, y: i32, colour: Colour) {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return;
        }
        if let Some((x0, y0, x1, y1)) = self.clip {
            if x < x0 || y < y0 || x >= x1 || y >= y1 {
                return;
            }
        }
        let (x, y) = match self.rotate {
            Rotate::Normal => (x as usize, y as usize),
            Rotate::Half => (
                (self.width as i32 - 1 - x) as usize,
                (self.height as i32 - 1 - y) as usize,
            ),
        };
        let w = self.width as usize;
        match self.format {
            Format::Paged1 => {
                let i = (y / 8) * w + x;
                let bit = 1u8 << (y % 8);
                if colour.on() {
                    self.bytes[i] |= bit;
                } else {
                    self.bytes[i] &= !bit;
                }
            }
            Format::Grey4 => {
                let i = y * w.div_ceil(2) + x / 2;
                let g = colour.grey4();
                if x % 2 == 0 {
                    self.bytes[i] = (self.bytes[i] & 0x0F) | (g << 4);
                } else {
                    self.bytes[i] = (self.bytes[i] & 0xF0) | g;
                }
            }
            Format::Rgb565 => {
                let i = (y * w + x) * 2;
                let [hi, lo] = colour.rgb565().to_be_bytes();
                self.bytes[i] = hi;
                self.bytes[i + 1] = lo;
            }
        }
    }

    /// Read one pixel back, as the format kept it.
    pub fn get(&self, x: i32, y: i32) -> Colour {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return Colour::BLACK;
        }
        let (x, y) = match self.rotate {
            Rotate::Normal => (x as usize, y as usize),
            Rotate::Half => (
                (self.width as i32 - 1 - x) as usize,
                (self.height as i32 - 1 - y) as usize,
            ),
        };
        let w = self.width as usize;
        match self.format {
            Format::Paged1 => {
                let on = self.bytes[(y / 8) * w + x] & (1 << (y % 8)) != 0;
                if on {
                    Colour::WHITE
                } else {
                    Colour::BLACK
                }
            }
            Format::Grey4 => {
                let byte = self.bytes[y * w.div_ceil(2) + x / 2];
                let g = if x % 2 == 0 { byte >> 4 } else { byte & 0x0F };
                Colour::grey(g << 4 | g)
            }
            Format::Rgb565 => {
                let i = (y * w + x) * 2;
                let v = u16::from_be_bytes([self.bytes[i], self.bytes[i + 1]]);
                let r = ((v >> 11) & 0x1F) as u8;
                let g = ((v >> 5) & 0x3F) as u8;
                let b = (v & 0x1F) as u8;
                Colour::rgb(r << 3 | r >> 2, g << 2 | g >> 4, b << 3 | b >> 2)
            }
        }
    }

    /// A filled rectangle, clipped to the canvas.
    pub fn fill_rect(&mut self, x: i32, y: i32, w: i32, h: i32, colour: Colour) {
        let x0 = x.max(0);
        let y0 = y.max(0);
        let x1 = (x + w).min(self.width as i32);
        let y1 = (y + h).min(self.height as i32);
        for yy in y0..y1 {
            for xx in x0..x1 {
                self.set(xx, yy, colour);
            }
        }
    }

    /// A one-pixel outline, clipped to the canvas.
    pub fn rect(&mut self, x: i32, y: i32, w: i32, h: i32, colour: Colour) {
        if w <= 0 || h <= 0 {
            return;
        }
        self.fill_rect(x, y, w, 1, colour);
        self.fill_rect(x, y + h - 1, w, 1, colour);
        self.fill_rect(x, y, 1, h, colour);
        self.fill_rect(x + w - 1, y, 1, h, colour);
    }

    /// A horizontal line from `x` for `len` pixels.
    pub fn hline(&mut self, x: i32, y: i32, len: i32, colour: Colour) {
        self.fill_rect(x, y, len, 1, colour);
    }

    /// A vertical line from `y` for `len` pixels.
    pub fn vline(&mut self, x: i32, y: i32, len: i32, colour: Colour) {
        self.fill_rect(x, y, 1, len, colour);
    }

    /// `text` in `font` with the cell's top-left at `(x, y)`; returns the
    /// pen's x after the last glyph. Characters the font lacks draw its mark.
    pub fn text(&mut self, font: &Font, x: i32, y: i32, text: &str, colour: Colour) -> i32 {
        let mut pen = x;
        for c in text.chars() {
            let Some(g) = font.glyph_or_mark(c) else {
                continue;
            };
            let gx = pen + g.left as i32;
            let gy = y + g.top as i32;
            for row in 0..g.height as u32 {
                for col in 0..g.width as u32 {
                    if font.lit(g, col, row) {
                        self.set(gx + col as i32, gy + row as i32, colour);
                    }
                }
            }
            pen += g.advance as i32;
        }
        pen
    }

    /// `text` centred in the span `x..x + width`, the cell's top at `y`.
    pub fn text_centred(
        &mut self,
        font: &Font,
        x: i32,
        y: i32,
        width: i32,
        text: &str,
        colour: Colour,
    ) {
        let w = font.width(text);
        self.text(font, x + (width - w) / 2, y, text, colour);
    }

    /// `text` with every glyph pixel drawn `scale` times over, for a big
    /// face out of a small one; returns the pen's x after the last glyph.
    pub fn text_scaled(
        &mut self,
        font: &Font,
        x: i32,
        y: i32,
        text: &str,
        colour: Colour,
        scale: i32,
    ) -> i32 {
        let scale = scale.max(1);
        let mut pen = x;
        for c in text.chars() {
            let Some(g) = font.glyph_or_mark(c) else {
                continue;
            };
            let gx = pen + g.left as i32 * scale;
            let gy = y + g.top as i32 * scale;
            for row in 0..g.height as u32 {
                for col in 0..g.width as u32 {
                    if font.lit(g, col, row) {
                        self.fill_rect(
                            gx + col as i32 * scale,
                            gy + row as i32 * scale,
                            scale,
                            scale,
                            colour,
                        );
                    }
                }
            }
            pen += g.advance as i32 * scale;
        }
        pen
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_fold_to_each_format() {
        assert_eq!(Colour::WHITE.rgb565(), 0xFFFF);
        assert_eq!(Colour::rgb(255, 0, 0).rgb565(), 0xF800);
        assert_eq!(Colour::rgb(0, 255, 0).rgb565(), 0x07E0);
        assert_eq!(Colour::rgb(0, 0, 255).rgb565(), 0x001F);
        assert_eq!(Colour::WHITE.luma(), 255);
        assert_eq!(Colour::BLACK.luma(), 0);
        assert_eq!(Colour::WHITE.grey4(), 15);
        assert_eq!(Colour::grey(0x80).grey4(), 8);
        assert!(Colour::WHITE.on());
        assert!(!Colour::grey(100).on());
        assert_eq!(Rotate::from_degrees(180), Rotate::Half);
        assert_eq!(Rotate::from_degrees(0), Rotate::Normal);
        assert_eq!(Rotate::from_degrees(90), Rotate::Normal);
    }

    #[test]
    fn a_paged_frame_puts_eight_rows_in_a_byte() {
        let mut c = Canvas::new(Format::Paged1, 128, 64);
        assert_eq!(c.bytes().len(), 1024);
        assert_eq!(c.pages(), 8);
        c.set(0, 0, Colour::WHITE);
        c.set(0, 7, Colour::WHITE);
        c.set(5, 9, Colour::WHITE);
        assert_eq!(c.bytes()[0], 0x81);
        assert_eq!(c.page(1)[5], 0x02);
        assert_eq!(c.get(0, 7), Colour::WHITE);
        assert_eq!(c.get(1, 7), Colour::BLACK);
        c.set(0, 0, Colour::BLACK);
        assert_eq!(c.bytes()[0], 0x80);
        c.fill(Colour::WHITE);
        assert!(c.bytes().iter().all(|&b| b == 0xFF));
        c.clear();
        assert!(c.bytes().iter().all(|&b| b == 0));
        assert_eq!(c.page(8), &[]);
    }

    #[test]
    fn a_grey_frame_puts_two_pixels_in_a_byte() {
        let mut c = Canvas::new(Format::Grey4, 256, 64);
        assert_eq!(c.bytes().len(), 8192);
        assert_eq!(c.pages(), 0);
        assert_eq!(c.page(0), &[]);
        c.set(0, 0, Colour::WHITE);
        c.set(1, 0, Colour::grey(0x80));
        assert_eq!(c.bytes()[0], 0xF8);
        c.set(2, 1, Colour::grey(0x30));
        assert_eq!(c.bytes()[128 + 1], 0x30);
        assert_eq!(c.get(1, 0), Colour::grey(0x88));
        c.fill(Colour::grey(0xA0));
        assert!(c.bytes().iter().all(|&b| b == 0xAA));
    }

    #[test]
    fn a_colour_frame_is_big_endian_rgb565() {
        let mut c = Canvas::new(Format::Rgb565, 128, 128);
        assert_eq!(c.bytes().len(), 32768);
        c.set(1, 0, Colour::rgb(255, 0, 0));
        assert_eq!(&c.bytes()[2..4], &[0xF8, 0x00]);
        c.set(0, 1, Colour::rgb(0, 0, 255));
        assert_eq!(&c.bytes()[256..258], &[0x00, 0x1F]);
        assert_eq!(c.get(1, 0), Colour::rgb(255, 0, 0));
        assert_eq!(c.get(0, 1), Colour::rgb(0, 0, 255));
        assert_eq!(c.get(0, 0), Colour::BLACK);
        c.fill(Colour::rgb(0, 255, 0));
        assert!(c
            .bytes()
            .as_chunks::<2>()
            .0
            .iter()
            .all(|p| *p == [0x07, 0xE0]));
    }

    #[test]
    fn half_a_turn_mirrors_both_axes_and_shapes_clip() {
        let mut c = Canvas::new(Format::Paged1, 128, 64).with_rotate(Rotate::Half);
        c.set(0, 0, Colour::WHITE);
        assert_eq!(c.bytes()[7 * 128 + 127], 0x80);
        assert_eq!(c.get(0, 0), Colour::WHITE);
        assert_eq!(c.get(127, 63), Colour::BLACK);

        let mut c = Canvas::new(Format::Paged1, 8, 8);
        c.rect(-2, -2, 6, 6, Colour::WHITE);
        // Only the right and bottom edges land on the canvas.
        assert_eq!(c.bytes()[3], 0x0F);
        assert_eq!(c.bytes()[0], 0x08);
        c.clear();
        c.fill_rect(6, 6, 10, 10, Colour::WHITE);
        assert_eq!(&c.bytes()[6..8], &[0xC0, 0xC0]);
        c.clear();
        c.hline(0, 7, 8, Colour::WHITE);
        c.vline(7, 0, 8, Colour::WHITE);
        assert_eq!(c.bytes()[7], 0xFF);
        assert_eq!(c.bytes()[0], 0x80);
        c.set(-1, 0, Colour::WHITE);
        c.set(8, 0, Colour::WHITE);
        assert_eq!(c.get(-1, 0), Colour::BLACK);
        // A clip box keeps a fill inside it until cleared.
        c.clear();
        c.set_clip(2, 2, 3, 3);
        c.fill_rect(0, 0, 8, 8, Colour::WHITE);
        assert_eq!(c.get(1, 1), Colour::BLACK);
        assert_eq!(c.get(2, 2), Colour::WHITE);
        assert_eq!(c.get(4, 4), Colour::WHITE);
        assert_eq!(c.get(5, 5), Colour::BLACK);
        c.clear_clip();
        c.set(0, 0, Colour::WHITE);
        assert_eq!(c.get(0, 0), Colour::WHITE);
    }

    #[test]
    fn text_lands_in_its_cell_and_centres() {
        use crate::font::MEDIUM;
        let mut c = Canvas::new(Format::Paged1, 128, 32);
        let end = c.text(MEDIUM, 2, 4, "Hi", Colour::WHITE);
        assert_eq!(end, 2 + MEDIUM.width("Hi"));
        // Ink within the cell: rows 4..24, nothing above or below.
        let lit = |c: &Canvas, y: i32| (0..128).any(|x| c.get(x, y) == Colour::WHITE);
        assert!((4..24).any(|y| lit(&c, y)));
        assert!(!(0..3).any(|y| lit(&c, y)));
        assert!(!(25..32).any(|y| lit(&c, y)));
        // The H's stem starts at the pen plus the glyph's left bearing.
        let h = MEDIUM.glyph('H').unwrap();
        assert_eq!(c.get(2 + h.left as i32, 4 + h.top as i32), Colour::WHITE);

        let mut c = Canvas::new(Format::Paged1, 128, 32);
        c.text_centred(MEDIUM, 0, 0, 128, "II", Colour::WHITE);
        let first = (0..128)
            .find(|&x| (0..32).any(|y| c.get(x, y) == Colour::WHITE))
            .unwrap();
        let last = (0..128)
            .rev()
            .find(|&x| (0..32).any(|y| c.get(x, y) == Colour::WHITE))
            .unwrap();
        assert!((first - (127 - last)).abs() <= 2, "{first} {last}");
    }
}
