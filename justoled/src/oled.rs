//! The driver layer: a panel opened on its bus, started with its
//! controller's sequence, and shown a frame. Keyed by the catalogue entry,
//! so the settings page and this file agree on what a panel is.
//!
//! Bytes go down one of two links. Four-wire SPI tells command from data
//! by the DC line; I2C by a control byte in front (0x00 command, 0x40
//! data), as the SSD1306 family and SSD1327 define it. Controllers of the
//! SSD1351 class take a command's arguments as data (DC high); the SSD1306
//! class takes them as further commands. Each sequence says which. The
//! TFT LCDs (ST7789, ST7735, ILI9341) speak the MIPI DCS set, arguments
//! as data, and add a backlight line and a quarter turn of their own.
//!
//! Sequences are transcribed from the controller data sheets and checked
//! against the module vendor's reference drivers; the entry's `verified`
//! flag in the catalogue says whether a module has confirmed them.

use std::io;
use std::thread::sleep;
use std::time::Duration;

use crate::canvas::{Canvas, Rotate};
use crate::catalog::{Bus, Controller, Entry, Format};
use crate::sys::{gpio, i2c, spi};

/// Where the panel is wired.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Wiring {
    Spi {
        bus: u8,
        cs: u8,
        /// BCM number of the data/command line.
        dc: u32,
        /// BCM number of the reset line, if wired.
        reset: Option<u32>,
        /// BCM number of a TFT's backlight line, if wired; an OLED has none.
        backlight: Option<u32>,
    },
    I2c {
        bus: u8,
        addr: u8,
        /// BCM number of the reset line, if the module brings one out.
        reset: Option<u32>,
    },
}

impl Wiring {
    pub fn bus(&self) -> Bus {
        match self {
            Wiring::Spi { .. } => Bus::Spi,
            Wiring::I2c { .. } => Bus::I2c,
        }
    }
}

/// The consumer name the GPIO lines are requested under.
const CONSUMER: &str = "just-oled";

/// The longest I2C write handed to the kernel in one go: the control byte
/// and this many data bytes.
const I2C_CHUNK: usize = 1024;

enum Link {
    Spi {
        dev: spi::Device,
        /// DC at index 0; the reset and backlight lines at the indices kept.
        lines: gpio::Outputs,
        reset: Option<usize>,
        backlight: Option<usize>,
    },
    I2c {
        dev: i2c::Device,
        reset: Option<gpio::Outputs>,
    },
}

impl Link {
    fn open(entry: &Entry, wiring: &Wiring) -> io::Result<Link> {
        let chip =
            || gpio::Chip::header().ok_or_else(|| io::Error::other("no GPIO chip under /dev"));
        let named = |what: String, e: io::Error| io::Error::new(e.kind(), format!("{what}: {e}"));
        match *wiring {
            Wiring::Spi {
                bus,
                cs,
                dc,
                reset,
                backlight,
            } => {
                let (mode, speed) = spi_settings(entry.controller);
                let dev = spi::Device::open(bus, cs, mode, speed)
                    .map_err(|e| named(format!("/dev/spidev{bus}.{cs}"), e))?;
                let chip = chip()?;
                // DC low; reset held high; the backlight dark until the panel is on.
                let mut offsets = vec![dc];
                let mut values = vec![false];
                let reset_index = reset.map(|rst| {
                    offsets.push(rst);
                    values.push(true);
                    offsets.len() - 1
                });
                let backlight_index = backlight.filter(|_| entry.controller.is_tft()).map(|bl| {
                    offsets.push(bl);
                    values.push(false);
                    offsets.len() - 1
                });
                let lines = gpio::Outputs::request(&chip.path, &offsets, &values, CONSUMER)
                    .map_err(|e| named(format!("{} lines {offsets:?}", chip.path), e))?;
                Ok(Link::Spi {
                    dev,
                    lines,
                    reset: reset_index,
                    backlight: backlight_index,
                })
            }
            Wiring::I2c { bus, addr, reset } => {
                let dev = i2c::Device::open(bus, addr)
                    .map_err(|e| named(format!("/dev/i2c-{bus} address {addr:#04x}"), e))?;
                let reset = match reset {
                    Some(rst) => {
                        let chip = chip()?;
                        Some(
                            gpio::Outputs::request(&chip.path, &[rst], &[true], CONSUMER)
                                .map_err(|e| named(format!("{} line {rst}", chip.path), e))?,
                        )
                    }
                    None => None,
                };
                Ok(Link::I2c { dev, reset })
            }
        }
    }

    /// Bytes with DC low.
    fn command(&self, bytes: &[u8]) -> io::Result<()> {
        match self {
            Link::Spi { dev, lines, .. } => {
                lines.set(0, false)?;
                dev.write(bytes)
            }
            Link::I2c { dev, .. } => i2c_stream(dev, 0x00, bytes),
        }
    }

    /// Bytes with DC high.
    fn data(&self, bytes: &[u8]) -> io::Result<()> {
        match self {
            Link::Spi { dev, lines, .. } => {
                lines.set(0, true)?;
                dev.write(bytes)
            }
            Link::I2c { dev, .. } => i2c_stream(dev, 0x40, bytes),
        }
    }

    /// Pulse reset low, where a line is wired; otherwise just wait for
    /// the panel's own power-on reset to have finished.
    fn reset(&self) -> io::Result<()> {
        let line = match self {
            Link::Spi {
                lines,
                reset: Some(index),
                ..
            } => Some((lines, *index)),
            Link::I2c {
                reset: Some(lines), ..
            } => Some((lines, 0)),
            _ => None,
        };
        if let Some((lines, index)) = line {
            lines.set(index, true)?;
            sleep(Duration::from_millis(1));
            lines.set(index, false)?;
            sleep(Duration::from_millis(10));
            lines.set(index, true)?;
        }
        sleep(Duration::from_millis(10));
        Ok(())
    }

    /// The backlight line, where one is wired.
    fn backlight(&self, on: bool) -> io::Result<()> {
        if let Link::Spi {
            lines,
            backlight: Some(index),
            ..
        } = self
        {
            lines.set(*index, on)?;
        }
        Ok(())
    }
}

fn i2c_stream(dev: &i2c::Device, control: u8, bytes: &[u8]) -> io::Result<()> {
    let mut buf = Vec::with_capacity(I2C_CHUNK + 1);
    for chunk in bytes.chunks(I2C_CHUNK) {
        buf.clear();
        buf.push(control);
        buf.extend_from_slice(chunk);
        dev.write(&buf)?;
    }
    Ok(())
}

/// SPI mode and clock for a controller: the mode its data sheet draws
/// (all of them sample on the rising edge, so mode 0), the clock a safe
/// step under the sheet's maximum.
fn spi_settings(controller: Controller) -> (u8, u32) {
    match controller {
        // 20 MHz maximum (50 ns cycle); 10 MHz is what the module vendor's demo runs.
        Controller::Ssd1351 => (0, 10_000_000),
        // Sino Wealth's sheets give a 250 ns cycle: 4 MHz.
        Controller::Sh1106 | Controller::Sh1107 | Controller::Sh1122 => (0, 4_000_000),
        // The TFTs take 16 ns cycles (62.5 MHz) writing; 40 MHz is what
        // Waveshare's demos run, and a 240x320 frame then takes 31 ms.
        Controller::St7789 | Controller::Ili9341 => (0, 40_000_000),
        // ST7735: 66 ns cycle, 15 MHz.
        Controller::St7735 => (0, 15_000_000),
        // Solomon Systech's one-bit and grey controllers: 100 ns cycle, 10 MHz; a step under.
        _ => (0, 8_000_000),
    }
}

/// A panel, opened and ready to be started.
pub struct Oled {
    entry: &'static Entry,
    link: Link,
    /// The turn a TFT makes in its controller; `Normal` on an OLED.
    rotate: Rotate,
}

impl std::fmt::Debug for Oled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Oled({})", self.entry.id)
    }
}

impl Oled {
    /// Open the bus and lines for `entry` as wired. Nothing is sent yet.
    pub fn open(entry: &'static Entry, wiring: &Wiring) -> io::Result<Oled> {
        if !entry.has_bus(wiring.bus()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{} is not an {} panel", entry.id, wiring.bus().name()),
            ));
        }
        if !driven_entry(entry) {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                if driven(entry.controller) {
                    format!("{} is catalogued but not yet driven at this size", entry.id)
                } else {
                    format!("{} is catalogued but not yet driven", entry.controller)
                },
            ));
        }
        let link = Link::open(entry, wiring)?;
        Ok(Oled {
            entry,
            link,
            rotate: Rotate::Normal,
        })
    }

    pub fn entry(&self) -> &'static Entry {
        self.entry
    }

    /// Take the turn the settings ask for. A TFT turns in its controller
    /// and this returns `Normal` for the canvas; an OLED turns in the
    /// canvas, which can only turn half way, so a quarter turn there
    /// shows as normal. Call before `init`.
    pub fn orient(&mut self, rotate: Rotate) -> Rotate {
        if self.entry.controller.is_tft() {
            self.rotate = rotate;
            Rotate::Normal
        } else {
            rotate.in_software()
        }
    }

    /// The frame's size as the panel shows it, after the controller's turn.
    pub fn size(&self) -> (u16, u16) {
        if self.rotate.swaps_sides() {
            (self.entry.height, self.entry.width)
        } else {
            (self.entry.width, self.entry.height)
        }
    }

    /// A blank frame of the panel's size and format.
    pub fn canvas(&self) -> Canvas {
        let (w, h) = self.size();
        Canvas::new(self.entry.format(), w, h)
    }

    /// Whether `show_rows` sends less than the whole frame: the TFTs
    /// take a row window, the OLEDs here a whole frame.
    pub fn windowed(&self) -> bool {
        self.entry.controller.is_tft()
    }

    /// Reset, run the controller's start-up sequence, clear the RAM and
    /// turn the display on.
    pub fn init(&mut self) -> io::Result<()> {
        self.link.reset()?;
        match family(self.entry.controller) {
            Family::Ssd1351 => ssd1351::init(&self.link, self.entry)?,
            Family::Ssd1331 => ssd1331::init(&self.link)?,
            Family::Ssd1306 => ssd1306::init(&self.link, self.entry)?,
            Family::Sh1106 => sh1106::init(&self.link, self.entry)?,
            Family::Sh1107 => sh1107::init(&self.link, self.entry)?,
            Family::Grey => grey::init(&self.link, self.entry)?,
            Family::Tft => tft::init(&self.link, self.entry, self.rotate)?,
            Family::None => return Err(not_driven(self.entry.controller)),
        }
        let blank = self.canvas();
        self.show(&blank)?;
        self.on()
    }

    /// Send a whole frame. The canvas must be the panel's own.
    pub fn show(&mut self, canvas: &Canvas) -> io::Result<()> {
        let (w, h) = self.size();
        if canvas.format() != self.entry.format() || canvas.width() != w || canvas.height() != h {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "the frame is not this panel's size",
            ));
        }
        match family(self.entry.controller) {
            Family::Ssd1351 => ssd1351::show(&self.link, self.entry, canvas.bytes()),
            Family::Ssd1331 => ssd1331::show(&self.link, self.entry, canvas.bytes()),
            Family::Ssd1306 => ssd1306::show(&self.link, self.entry, canvas),
            Family::Sh1106 | Family::Sh1107 => paged::show_by_page(&self.link, self.entry, canvas),
            Family::Grey => grey::show(&self.link, self.entry, canvas),
            Family::Tft => tft::show_rows(&self.link, self.entry, self.rotate, canvas, 0..h),
            Family::None => Err(not_driven(self.entry.controller)),
        }
    }

    /// Send the rows `first..end` of the frame, where the controller takes
    /// a window; the whole frame otherwise.
    pub fn show_rows(&mut self, canvas: &Canvas, rows: std::ops::Range<u16>) -> io::Result<()> {
        let (w, h) = self.size();
        if !self.windowed() || (rows.start == 0 && rows.end >= h) {
            return self.show(canvas);
        }
        if canvas.format() != self.entry.format() || canvas.width() != w || canvas.height() != h {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "the frame is not this panel's size",
            ));
        }
        let rows = rows.start.min(h)..rows.end.min(h);
        if rows.is_empty() {
            return Ok(());
        }
        tft::show_rows(&self.link, self.entry, self.rotate, canvas, rows)
    }

    pub fn on(&mut self) -> io::Result<()> {
        match family(self.entry.controller) {
            Family::None => Err(not_driven(self.entry.controller)),
            Family::Tft => {
                self.link.command(&[tft::DISPON])?;
                self.link.backlight(true)
            }
            // The same byte on every OLED controller here.
            _ => self.link.command(&[DISPLAY_ON]),
        }
    }

    /// Display off: the panel goes dark and its RAM keeps the frame.
    pub fn off(&mut self) -> io::Result<()> {
        match family(self.entry.controller) {
            Family::None => Err(not_driven(self.entry.controller)),
            Family::Tft => {
                self.link.backlight(false)?;
                self.link.command(&[tft::DISPOFF])
            }
            _ => self.link.command(&[DISPLAY_OFF]),
        }
    }

    /// Brightness 0..=255 as the controller counts it. A TFT's brightness
    /// is its backlight, which the line here can only switch.
    pub fn contrast(&mut self, level: u8) -> io::Result<()> {
        match family(self.entry.controller) {
            Family::Ssd1351 => ssd1351::contrast(&self.link, level),
            Family::Ssd1331 => self.link.command(&[0x81, level, 0x82, level, 0x83, level]),
            Family::Ssd1306 | Family::Sh1106 | Family::Sh1107 => {
                self.link.command(&[paged::CONTRAST, level])
            }
            Family::Grey => grey::contrast(&self.link, self.entry, level),
            Family::Tft => Ok(()),
            Family::None => Err(not_driven(self.entry.controller)),
        }
    }
}

/// Display on and off are 0xAF and 0xAE on every OLED controller driven here.
const DISPLAY_ON: u8 = 0xAF;
const DISPLAY_OFF: u8 = 0xAE;

/// The command set a controller speaks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Family {
    /// SSD1306 and its near relatives: horizontal addressing, one burst per frame.
    Ssd1306,
    /// Page addressing in a 132-column RAM.
    Sh1106,
    /// Page addressing, pages along the long axis.
    Sh1107,
    /// The four-bit greys: SSD1322, SSD1325, SSD1327, SSD1362, SH1122.
    Grey,
    Ssd1351,
    /// 96x64 RGB; arguments as commands, unlike the SSD1351.
    Ssd1331,
    /// The TFT LCDs on the MIPI DCS command set: ST7789, ST7735, ILI9341.
    Tft,
    None,
}

fn family(controller: Controller) -> Family {
    match controller {
        Controller::Ssd1305 | Controller::Ssd1306 | Controller::Ssd1309 | Controller::Ssd1315 => {
            Family::Ssd1306
        }
        Controller::Sh1106 => Family::Sh1106,
        Controller::Sh1107 => Family::Sh1107,
        Controller::Ssd1322
        | Controller::Ssd1325
        | Controller::Ssd1327
        | Controller::Ssd1362
        | Controller::Sh1122 => Family::Grey,
        Controller::Ssd1351 => Family::Ssd1351,
        Controller::Ssd1331 => Family::Ssd1331,
        Controller::St7789 | Controller::St7735 | Controller::Ili9341 => Family::Tft,
        _ => Family::None,
    }
}

/// Whether a sequence exists for the controller yet.
pub fn driven(controller: Controller) -> bool {
    family(controller) != Family::None
}

/// Whether a sequence exists for this very entry: a controller may be
/// driven while one of its glasses is not (the SH1107 128x64 is a 64x128
/// turned on its side, which needs the quarter turn the canvas lacks).
pub fn driven_entry(entry: &Entry) -> bool {
    driven(entry.controller) && entry.id != "sh1107_128x64"
}

fn not_driven(controller: Controller) -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        format!("{controller} is catalogued but not yet driven"),
    )
}

/// What the one-bit controllers share: the page-addressed frame write
/// and the geometry each glass sits at in its controller's RAM.
/// Transcribed from the data sheets, u8g2's `u8x8_d_*.c` drivers (BSD-2,
/// olikraus) and luma.oled where they agree, each noted where they do not.
mod paged {
    use super::*;

    pub const CONTRAST: u8 = 0x81;

    /// Where a glass sits in its controller's RAM.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct Geometry {
        /// 0xA8: rows driven, less one.
        pub multiplex: u8,
        /// 0xD3: the COM line the first row starts at.
        pub row_offset: u8,
        /// The RAM column the glass's first column is.
        pub col_offset: u8,
        /// 0xDA: COM pins wiring, 0x12 alternative or 0x02 sequential.
        pub com_pins: u8,
    }

    pub fn geometry(entry: &Entry) -> Geometry {
        let mux = (entry.height - 1) as u8;
        match entry.id {
            // u8g2 ssd1306_64x48, ssd1306_64x32: the middle 64 columns of the 128.
            "ssd1306_64x48" | "ssd1306_64x32" => Geometry {
                multiplex: mux,
                row_offset: 0,
                col_offset: 32,
                com_pins: 0x12,
            },
            // u8g2 ssd1306_72x40: column offset 28 on the 0.42" glass.
            "ssd1306_72x40" => Geometry {
                multiplex: mux,
                row_offset: 0,
                col_offset: 28,
                com_pins: 0x12,
            },
            // u8g2 ssd1305 (noname, Adafruit 2675): the glass on COM 32 up, column 4 in.
            "ssd1305_128x32" => Geometry {
                multiplex: 0x3F,
                row_offset: 32,
                col_offset: 4,
                com_pins: 0x12,
            },
            // 132-column RAM, the glass two columns in (u8g2, luma, every 1.3" module page).
            "sh1106_128x64" => Geometry {
                multiplex: mux,
                row_offset: 0,
                col_offset: 2,
                com_pins: 0x12,
            },
            // u8g2 and luma agree: the 64-column glass sits at display offset 0x60.
            "sh1107_64x128" => Geometry {
                multiplex: 0x7F,
                row_offset: 0x60,
                col_offset: 0,
                com_pins: 0x12,
            },
            _ => Geometry {
                multiplex: mux,
                row_offset: 0,
                col_offset: 0,
                // 16- and 32-row glasses wire their COM lines sequentially.
                com_pins: if entry.height <= 32 { 0x02 } else { 0x12 },
            },
        }
    }

    /// A one-bit frame a page at a time: page address, column low and
    /// high nibbles, then the page's bytes. The way SH1106 and SH1107
    /// take it, and a fallback for everything with a 0xB0 page command.
    pub fn show_by_page(link: &Link, entry: &Entry, canvas: &Canvas) -> io::Result<()> {
        debug_assert_eq!(entry.format(), Format::Paged1);
        let col = geometry(entry).col_offset;
        for page in 0..canvas.pages() {
            link.command(&[0xB0 | page as u8, col & 0x0F, 0x10 | (col >> 4)])?;
            link.data(canvas.page(page))?;
        }
        Ok(())
    }
}

/// SSD1306, and SSD1305, SSD1309 and SSD1315 with it: Solomon Systech's
/// sheets, u8g2's `u8x8_d_ssd1306_*.c`, `u8x8_d_ssd1305.c`,
/// `u8x8_d_ssd1309.c` and luma.oled's `ssd1306`. Command arguments are
/// commands themselves. The frame goes as one burst in horizontal
/// addressing mode over the glass's column and page window.
mod ssd1306 {
    use super::paged::geometry;
    use super::*;

    pub fn init(link: &Link, entry: &Entry) -> io::Result<()> {
        let g = geometry(entry);
        let (clock, precharge, vcomh, contrast) = match entry.controller {
            // u8g2 ssd1309: no charge pump; its own electrical values.
            Controller::Ssd1309 => (0xA0, 0xD3, 0x20, 0x6F),
            _ => match entry.id {
                // The small glasses' sheets (u8g2 64x48, 64x32, 72x40).
                "ssd1306_64x48" | "ssd1306_64x32" => (0x80, 0x22, 0x00, 0xCF),
                "ssd1306_72x40" => (0x80, 0x22, 0x20, 0xCF),
                "ssd1306_96x16" => (0x80, 0xF1, 0x20, 0xCF),
                "ssd1306_128x32" => (0x80, 0xF1, 0x40, 0x8F),
                _ => (0x80, 0xF1, 0x40, 0xCF),
            },
        };
        let mut seq: Vec<u8> = vec![
            DISPLAY_OFF,
            0xD5,
            clock,
            0xA8,
            g.multiplex,
            0xD3,
            g.row_offset,
            0x40,
        ];
        match entry.controller {
            // SSD1309 has no charge pump; SSD1305 runs from an external VCC.
            Controller::Ssd1309 => {}
            Controller::Ssd1305 => seq.extend_from_slice(&[0x8D, 0x10]),
            _ => seq.extend_from_slice(&[0x8D, 0x14]),
        }
        seq.extend_from_slice(&[
            0x20, 0x00, // horizontal addressing
            0xA1, // segment remap: column 127 to SEG0
            0xC8, // COM scan from COM[N-1] to COM0
            0xDA, g.com_pins, 0x81, contrast, 0xD9, precharge, 0xDB, vcomh,
            0x2E, // scrolling off
            0xA4, // show the RAM
            0xA6, // normal, not inverted
        ]);
        link.command(&seq)
    }

    pub fn show(link: &Link, entry: &Entry, canvas: &Canvas) -> io::Result<()> {
        debug_assert_eq!(entry.format(), Format::Paged1);
        let g = geometry(entry);
        let first = g.col_offset;
        let last = first + (entry.width - 1) as u8;
        let pages = canvas.pages() as u8;
        link.command(&[0x21, first, last, 0x22, 0x00, pages - 1])?;
        link.data(canvas.bytes())
    }
}

/// SH1106: Sino Wealth's sheet, u8g2's `u8x8_d_sh1106_*.c` and luma.oled's
/// `sh1106`. Page addressing only; the built-in DC-DC switched on.
mod sh1106 {
    use super::paged::geometry;
    use super::*;

    pub fn init(link: &Link, entry: &Entry) -> io::Result<()> {
        let g = geometry(entry);
        link.command(&[
            DISPLAY_OFF,
            0xD5,
            0x80, // clock divide and oscillator
            0xA8,
            g.multiplex,
            0xD3,
            g.row_offset,
            0x40, // start line 0
            0xAD,
            0x8B, // built-in DC-DC on
            0x33, // charge pump voltage 9 V (u8g2; the sheet's reset is 0x32)
            0xA1, // segment remap
            0xC8, // COM scan reversed
            0xDA,
            g.com_pins,
            0x81,
            0x80, // contrast
            0xD9,
            0x22, // precharge
            0xDB,
            0x35, // VCOMH
            0xA4,
            0xA6,
        ])
    }
}

/// SH1107: Sino Wealth's sheet, u8g2's `u8x8_d_sh1107.c` and luma.oled's
/// `sh1107`. Pages run along the long axis, so a 64x128 glass is sixteen
/// pages of 64 bytes, as the canvas holds it.
mod sh1107 {
    use super::paged::geometry;
    use super::*;

    pub fn init(link: &Link, entry: &Entry) -> io::Result<()> {
        let g = geometry(entry);
        link.command(&[
            DISPLAY_OFF,
            0xDC,
            0x00, // start line
            0x81,
            0x2F, // contrast (u8g2; luma 0x7F)
            0x20, // page addressing
            0xA0, // segment remap off
            0xC0, // COM scan normal
            0xA8,
            g.multiplex,
            0xD3,
            g.row_offset,
            0xD5,
            0x51, // clock divide and oscillator
            0xD9,
            0x22, // precharge
            0xDB,
            0x35, // VCOMH
            0xDA,
            g.com_pins,
            0xA4,
            0xA6,
        ])
    }
}

/// The four-bit grey controllers. Two pixels a byte, the left one in the
/// high nibble as the canvas packs it; each controller's remap bits are
/// set so that is how it reads them (u8g2's and luma.oled's choice for
/// the same packing). The SSD1322 takes command arguments as data, the
/// others as commands. Transcribed from the data sheets, u8g2's
/// `u8x8_d_ssd1322.c`, `u8x8_d_ssd1325.c`, `u8x8_d_ssd1327.c` and
/// `u8x8_d_sh1122.c` (BSD-2, olikraus) and luma.oled's `ssd1322`,
/// `ssd1327` and `ssd1362`.
mod grey {
    use super::*;

    /// Where a glass sits in its controller's RAM, in that controller's units.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct Geometry {
        /// 0xA8 or 0xCA: rows driven, less one.
        pub multiplex: u8,
        /// 0xA2: the COM line the first row starts at.
        pub row_offset: u8,
        /// The first column address of the glass, in the controller's
        /// column units (two pixels on most, four on the SSD1322).
        pub col_start: u8,
        /// The last column address of the glass.
        pub col_end: u8,
    }

    pub fn geometry(entry: &Entry) -> Geometry {
        let mux = (entry.height - 1) as u8;
        let per_address: u16 = if entry.controller == Controller::Ssd1322 {
            4
        } else {
            2
        };
        let columns = (entry.width / per_address) as u8;
        match entry.id {
            // u8g2 ssd1322_nhd_256x64, luma: 480 columns of RAM, the glass centred, 28 addresses in.
            "ssd1322_256x64" => Geometry {
                multiplex: 0x3F,
                row_offset: 0,
                col_start: 0x1C,
                col_end: 0x1C + columns - 1,
            },
            // u8g2 ssd1325_nhd_128x64: COM offset 0x4C.
            "ssd1325_128x64" => Geometry {
                multiplex: 0x3F,
                row_offset: 0x4C,
                col_start: 0,
                col_end: columns - 1,
            },
            // u8g2 visionox_128x96: 96 rows at COM offset 0x20.
            "ssd1327_128x96" => Geometry {
                multiplex: 0x5F,
                row_offset: 0x20,
                col_start: 0,
                col_end: columns - 1,
            },
            // u8g2 seeed_96x96: the glass at the far end of the 128-column RAM, COM offset 0x20.
            "ssd1327_96x96" => Geometry {
                multiplex: 0x5F,
                row_offset: 0x20,
                col_start: 16,
                col_end: 16 + columns - 1,
            },
            _ => Geometry {
                multiplex: mux,
                row_offset: 0,
                col_start: 0,
                col_end: columns - 1,
            },
        }
    }

    /// A command with its arguments, the way the controller takes them.
    fn cmd(link: &Link, entry: &Entry, command: u8, args: &[u8]) -> io::Result<()> {
        if entry.controller == Controller::Ssd1322 {
            link.command(&[command])?;
            if !args.is_empty() {
                link.data(args)?;
            }
            Ok(())
        } else {
            let mut bytes = Vec::with_capacity(1 + args.len());
            bytes.push(command);
            bytes.extend_from_slice(args);
            link.command(&bytes)
        }
    }

    pub fn init(link: &Link, entry: &Entry) -> io::Result<()> {
        let g = geometry(entry);
        let c = |command: u8, args: &[u8]| cmd(link, entry, command, args);
        match entry.controller {
            // u8g2's ws_128x128 order, from Waveshare's demo for its 1.5inch OLED Module.
            Controller::Ssd1327 => {
                c(DISPLAY_OFF, &[])?;
                c(0x15, &[g.col_start, g.col_end])?;
                c(0x75, &[0x00, mux_rows(entry)])?;
                c(0x81, &[0x80])?; // contrast
                c(0xA0, &[0x51])?; // remap: column remap, COM remap, COM split; nibbles as written
                c(0xA1, &[0x00])?; // start line
                c(0xA2, &[g.row_offset])?;
                c(0xA4, &[])?; // normal display
                c(0xA8, &[g.multiplex])?;
                c(0xB1, &[0xF1])?; // phase length
                c(0xB3, &[0x00])?; // clock divide and oscillator
                c(0xAB, &[0x01])?; // internal VDD regulator
                c(0xB6, &[0x0F])?; // second precharge period
                c(0xBE, &[0x0F])?; // VCOMH
                c(0xBC, &[0x08])?; // precharge voltage
                c(0xD5, &[0x62])?; // second precharge and VSL
                c(0xFD, &[0x12])?; // command lock: unlocked
            }
            // luma.oled's ssd1322 (the common 3.12" 256x64 glasses, NHD's sheet); u8g2 agrees but for the flip.
            Controller::Ssd1322 => {
                c(0xFD, &[0x12])?; // unlock
                c(DISPLAY_OFF, &[])?;
                c(0xB3, &[0x91])?; // clock divide and oscillator
                c(0xCA, &[g.multiplex])?;
                c(0xA2, &[g.row_offset])?;
                c(0xA1, &[0x00])?; // start line
                c(0xA0, &[0x14, 0x11])?; // remap: nibble remap, COM scan reversed; dual COM line
                c(0xB5, &[0x00])?; // GPIO off
                c(0xAB, &[0x01])?; // internal VDD regulator
                c(0xB4, &[0xA0, 0xFD])?; // display enhancement A: external VSL
                c(0xC1, &[0x9F])?; // contrast current
                c(0xC7, &[0x0F])?; // master contrast at full
                c(0xB9, &[])?; // linear grey scale
                c(0xB1, &[0xE2])?; // phase length
                c(0xD1, &[0x82, 0x20])?; // display enhancement B
                c(0xBB, &[0x1F])?; // precharge voltage
                c(0xB6, &[0x08])?; // second precharge period
                c(0xBE, &[0x07])?; // VCOMH
                c(0xA6, &[])?; // normal display
                c(0xA9, &[])?; // exit partial display
            }
            // u8g2 ssd1325_nhd_128x64.
            Controller::Ssd1325 => {
                c(DISPLAY_OFF, &[])?;
                c(0xB3, &[0x91])?; // clock divide and oscillator
                c(0xA8, &[g.multiplex])?;
                c(0xA2, &[g.row_offset])?;
                c(0xA1, &[0x00])?; // start line
                c(0xAD, &[0x02])?; // master configuration: external VCC
                c(0xA0, &[0x52])?; // remap: nibble remap on, COM remap, COM split
                c(0x86, &[])?; // full current range
                c(0xB8, &[0x01, 0x11, 0x22, 0x32, 0x43, 0x54, 0x65, 0x76])?; // grey scale table
                c(0x81, &[0x70])?; // contrast
                c(0xB2, &[0x51])?; // frame frequency
                c(0xB1, &[0x55])?; // phase length
                c(0xBC, &[0x10])?; // precharge voltage
                c(0xBE, &[0x1C])?; // VCOMH
                c(0xA4, &[])?; // normal display
            }
            // luma.oled's ssd1362, the nibble remap bit cleared for the canvas's packing.
            Controller::Ssd1362 => {
                c(0xAB, &[0x01])?; // internal VDD regulator
                c(0xAD, &[0x9E])?; // IREF selection
                c(0x15, &[g.col_start, g.col_end])?;
                c(0x75, &[0x00, mux_rows(entry)])?;
                c(0xA0, &[0x41])?; // remap: column remap, COM split
                c(0xA1, &[0x00])?; // start line
                c(0xA2, &[g.row_offset])?;
                c(0xA4, &[])?; // normal display
                c(0xA8, &[g.multiplex])?;
                c(0xB1, &[0x11])?; // phase lengths
                c(0xB3, &[0xF0])?; // clock divide and oscillator
                c(0xB9, &[])?; // linear grey scale
                c(0xBC, &[0x04])?; // precharge voltage
                c(0xBE, &[0x05])?; // VCOMH
            }
            // u8g2 sh1122_256x64.
            Controller::Sh1122 => {
                c(DISPLAY_OFF, &[])?;
                c(0x40, &[])?; // start line 0
                c(0xA0, &[])?; // segment remap off
                c(0xC0, &[])?; // COM scan normal
                c(0x81, &[0x80])?; // contrast
                c(0xA8, &[g.multiplex])?;
                c(0xAD, &[0x81])?; // built-in DC-DC, 0.6 x 500 kHz
                c(0xD5, &[0x50])?; // clock divide and oscillator
                c(0xD3, &[g.row_offset])?;
                c(0xD9, &[0x22])?; // precharge and discharge periods
                c(0xDB, &[0x35])?; // VCOM deselect level
                c(0xDC, &[0x35])?; // precharge output voltage
                c(0x30, &[])?; // discharge level
            }
            _ => return Err(not_driven(entry.controller)),
        }
        Ok(())
    }

    fn mux_rows(entry: &Entry) -> u8 {
        (entry.height - 1) as u8
    }

    pub fn contrast(link: &Link, entry: &Entry, level: u8) -> io::Result<()> {
        match entry.controller {
            Controller::Ssd1322 => cmd(link, entry, 0xC1, &[level]),
            _ => cmd(link, entry, 0x81, &[level]),
        }
    }

    pub fn show(link: &Link, entry: &Entry, canvas: &Canvas) -> io::Result<()> {
        debug_assert_eq!(entry.format(), Format::Grey4);
        let g = geometry(entry);
        match entry.controller {
            // Row addressed: each row set by hand, then its bytes.
            Controller::Sh1122 => {
                let row_bytes = (entry.width as usize).div_ceil(2);
                let col = g.col_start;
                for (y, row) in canvas.bytes().chunks(row_bytes).enumerate() {
                    link.command(&[0xB0, y as u8, col & 0x0F, 0x10 | (col >> 4)])?;
                    link.data(row)?;
                }
                Ok(())
            }
            _ => {
                cmd(link, entry, 0x15, &[g.col_start, g.col_end])?;
                cmd(link, entry, 0x75, &[0x00, mux_rows(entry)])?;
                if entry.controller == Controller::Ssd1322 {
                    link.command(&[0x5C])?; // write RAM
                }
                link.data(canvas.bytes())
            }
        }
    }
}

/// SSD1331: 96 x 64 RGB, 65K colour, after luma.oled's `ssd1331` and
/// Solomon Systech's sheet. Arguments go as commands; pixels follow the
/// window with no write command.
mod ssd1331 {
    use super::*;

    pub fn init(link: &Link) -> io::Result<()> {
        link.command(&[
            DISPLAY_OFF,
            0xA0,
            0x72, // remap: 65K colour, COM split, COM remap, column remap, RGB order
            0xA1,
            0x00, // start line
            0xA2,
            0x00, // display offset
            0xA4, // normal display
            0xA8,
            0x3F, // multiplex 64
            0xAD,
            0x8E, // master configuration: external VCC
            0xB0,
            0x0B, // power save off
            0xB1,
            0x74, // phase 1 and 2 periods
            0xB3,
            0xD0, // clock divide and oscillator
            0x8A,
            0x80,
            0x8B,
            0x80,
            0x8C,
            0x80, // precharge speed A, B, C
            0xBB,
            0x3E, // precharge voltage
            0xBE,
            0x3E, // VCOMH
            0x87,
            0x0F, // master current at full
            0x81,
            0xFF,
            0x82,
            0xFF,
            0x83,
            0xFF, // contrast A, B, C
        ])
    }

    pub fn show(link: &Link, entry: &Entry, frame: &[u8]) -> io::Result<()> {
        debug_assert_eq!(entry.format(), Format::Rgb565);
        link.command(&[
            0x15,
            0x00,
            (entry.width - 1) as u8,
            0x75,
            0x00,
            (entry.height - 1) as u8,
        ])?;
        link.data(frame)
    }
}

/// SSD1351: 128 x 128 RGB, 65K colour, in Solomon Systech's data sheet
/// rev 1.5. Command arguments go as data. Values follow the sheet's
/// reset defaults where the reference drivers (Waveshare's demo for the
/// 1.5inch RGB OLED Module, luma.oled) agree, theirs where they differ.
mod ssd1351 {
    use super::*;

    /// Remap and colour depth (0xA0): 65K colour, COM split odd/even,
    /// scan COM[N-1] to COM0, colour order C-B-A, horizontal increment.
    /// Waveshare's demo and most 1.5" modules want 0x74; a module whose
    /// red and blue come out swapped wants 0x70.
    const REMAP: u8 = 0x74;

    fn cmd(link: &Link, command: u8, args: &[u8]) -> io::Result<()> {
        link.command(&[command])?;
        if !args.is_empty() {
            link.data(args)?;
        }
        Ok(())
    }

    pub fn init(link: &Link, entry: &Entry) -> io::Result<()> {
        let last_col = (entry.width - 1) as u8;
        let last_row = (entry.height - 1) as u8;
        cmd(link, 0xFD, &[0x12])?; // unlock the command interface
        cmd(link, 0xFD, &[0xB1])?; // unlock A2, B1, B3, BB, BE, C1
        cmd(link, DISPLAY_OFF, &[])?;
        cmd(link, 0xB3, &[0xF1])?; // clock divide 1, oscillator 15
        cmd(link, 0xCA, &[last_row])?; // multiplex ratio: the rows there are
        cmd(link, 0x15, &[0x00, last_col])?; // column window
        cmd(link, 0x75, &[0x00, last_row])?; // row window
        cmd(link, 0xA0, &[REMAP])?;
        cmd(link, 0xA1, &[0x00])?; // start line
        cmd(link, 0xA2, &[0x00])?; // display offset
        cmd(link, 0xB5, &[0x00])?; // GPIO pins off
        cmd(link, 0xAB, &[0x01])?; // internal VDD regulator
        cmd(link, 0xB1, &[0x32])?; // phase 1 and 2 periods
        cmd(link, 0xB4, &[0xA0, 0xB5, 0x55])?; // segment low voltage: external VSL
        cmd(link, 0xBE, &[0x05])?; // VCOMH 0.82 x VCC
        cmd(link, 0xC7, &[0x0F])?; // master contrast at full
        cmd(link, 0xB6, &[0x01])?; // second precharge period
        cmd(link, 0xA6, &[])?; // normal display
        contrast(link, 0xFF)
    }

    pub fn contrast(link: &Link, level: u8) -> io::Result<()> {
        cmd(link, 0xC1, &[level, level, level])
    }

    pub fn show(link: &Link, entry: &Entry, frame: &[u8]) -> io::Result<()> {
        debug_assert_eq!(entry.format(), Format::Rgb565);
        cmd(link, 0x15, &[0x00, (entry.width - 1) as u8])?;
        cmd(link, 0x75, &[0x00, (entry.height - 1) as u8])?;
        link.command(&[0x5C])?; // write RAM
        link.data(frame)
    }
}

/// The TFT LCDs on the MIPI DCS command set: ST7789, ST7735 and ILI9341.
/// One RAM larger than most glasses, a window set by column and row
/// address before each write, and a memory access control register that
/// turns the picture any quarter turn and swaps the colour order.
/// Transcribed from the controller data sheets, luma.lcd's `device.py`
/// (MIT, Richard Hull and contributors) and Adafruit's ST7789 and ILI9341
/// drivers (MIT); the per-glass offsets from Adafruit's.
mod tft {
    use super::*;

    const SWRESET: u8 = 0x01;
    const SLPOUT: u8 = 0x11;
    const NORON: u8 = 0x13;
    const INVOFF: u8 = 0x20;
    const INVON: u8 = 0x21;
    pub const DISPOFF: u8 = 0x28;
    pub const DISPON: u8 = 0x29;
    const CASET: u8 = 0x2A;
    const RASET: u8 = 0x2B;
    const RAMWR: u8 = 0x2C;
    const MADCTL: u8 = 0x36;
    const COLMOD: u8 = 0x3A;

    /// MADCTL bits: row address order, column address order, row/column
    /// exchange, BGR colour order.
    const MY: u8 = 0x80;
    const MX: u8 = 0x40;
    const MV: u8 = 0x20;
    const BGR: u8 = 0x08;

    /// Where a glass sits in its controller's RAM, in the controller's own
    /// orientation (MADCTL 0), and what else it needs.
    struct Geometry {
        ram_w: u16,
        ram_h: u16,
        col_off: u16,
        row_off: u16,
        /// The glass wires blue where the controller says red.
        bgr: bool,
        /// The glass shows the RAM inverted, so the controller inverts it back.
        invert: bool,
        /// The glass scans columns the other way round from the ST7789
        /// glasses, so every turn carries MX the other way (the ILI9341:
        /// Adafruit's 0x48, 0x28, 0x88, 0xE8). Its window fills the RAM,
        /// so the origin does not move with it.
        mirror_x: bool,
    }

    fn geometry(entry: &Entry) -> Geometry {
        let (ram_w, ram_h) = match entry.controller {
            Controller::St7735 => (132, 162),
            _ => (240, 320),
        };
        let (col_off, row_off, bgr, invert) = match entry.id {
            "st7789_135x240" => (52, 40, false, true),
            "st7789_170x320" => (35, 0, false, true),
            "st7735_80x160" => (26, 1, true, true),
            "st7735_128x160" => (0, 0, true, false),
            "ili9341_240x320" => (0, 0, true, false),
            // The 240x320 and 240x240 ST7789 glasses start at the RAM's origin.
            _ => (0, 0, false, true),
        };
        Geometry {
            ram_w,
            ram_h,
            col_off,
            row_off,
            bgr,
            invert,
            mirror_x: entry.controller == Controller::Ili9341,
        }
    }

    /// The MADCTL byte and the window's origin for a turn: the window's
    /// far corners move with the glass as the addressing flips.
    pub(super) fn placement(entry: &Entry, rotate: Rotate) -> (u8, u16, u16) {
        let g = geometry(entry);
        let (w, h) = (entry.width, entry.height);
        let colour = if g.bgr { BGR } else { 0 };
        let mirror = if g.mirror_x { MX } else { 0 };
        match rotate {
            Rotate::Normal => (colour ^ mirror, g.col_off, g.row_off),
            Rotate::Half => (
                (MX | MY | colour) ^ mirror,
                g.ram_w - w - g.col_off,
                g.ram_h - h - g.row_off,
            ),
            Rotate::Quarter => (
                (MX | MV | colour) ^ mirror,
                g.row_off,
                g.ram_w - w - g.col_off,
            ),
            Rotate::ThreeQuarter => (
                (MY | MV | colour) ^ mirror,
                g.ram_h - h - g.row_off,
                g.col_off,
            ),
        }
    }

    fn cmd(link: &Link, command: u8, args: &[u8]) -> io::Result<()> {
        link.command(&[command])?;
        if !args.is_empty() {
            link.data(args)?;
        }
        Ok(())
    }

    pub fn init(link: &Link, entry: &Entry, rotate: Rotate) -> io::Result<()> {
        let g = geometry(entry);
        let (madctl, _, _) = placement(entry, rotate);
        cmd(link, SWRESET, &[])?;
        sleep(Duration::from_millis(150));
        cmd(link, SLPOUT, &[])?;
        sleep(Duration::from_millis(120));
        match entry.controller {
            Controller::St7789 => {
                cmd(link, COLMOD, &[0x55])?; // 16 bits a pixel
                cmd(link, 0xB2, &[0x0C, 0x0C, 0x00, 0x33, 0x33])?; // porch
                cmd(link, 0xB7, &[0x35])?; // gate voltages
                cmd(link, 0xBB, &[0x19])?; // VCOM 0.725 V
                cmd(link, 0xC0, &[0x2C])?; // LCM control
                cmd(link, 0xC2, &[0x01])?; // VDV and VRH from the registers
                cmd(link, 0xC3, &[0x12])?; // VRH
                cmd(link, 0xC4, &[0x20])?; // VDV
                cmd(link, 0xC6, &[0x0F])?; // 60 Hz
                cmd(link, 0xD0, &[0xA4, 0xA1])?; // power control 1
                cmd(
                    link,
                    0xE0,
                    &[
                        0xD0, 0x04, 0x0D, 0x11, 0x13, 0x2B, 0x3F, 0x54, 0x4C, 0x18, 0x0D, 0x0B,
                        0x1F, 0x23,
                    ],
                )?; // positive gamma
                cmd(
                    link,
                    0xE1,
                    &[
                        0xD0, 0x04, 0x0C, 0x11, 0x13, 0x2C, 0x3F, 0x44, 0x51, 0x2F, 0x1F, 0x1F,
                        0x20, 0x23,
                    ],
                )?; // negative gamma
            }
            Controller::St7735 => {
                cmd(link, 0xB1, &[0x01, 0x2C, 0x2D])?; // frame rate, normal
                cmd(link, 0xB2, &[0x01, 0x2C, 0x2D])?; // frame rate, idle
                cmd(link, 0xB3, &[0x01, 0x2C, 0x2D, 0x01, 0x2C, 0x2D])?; // frame rate, partial
                cmd(link, 0xB4, &[0x07])?; // no display inversion control
                cmd(link, 0xC0, &[0xA2, 0x02, 0x84])?; // power control 1
                cmd(link, 0xC1, &[0xC5])?; // power control 2
                cmd(link, 0xC2, &[0x0A, 0x00])?; // power control 3
                cmd(link, 0xC3, &[0x8A, 0x2A])?; // power control 4
                cmd(link, 0xC4, &[0x8A, 0xEE])?; // power control 5
                cmd(link, 0xC5, &[0x0E])?; // VCOM
                cmd(link, COLMOD, &[0x05])?; // 16 bits a pixel
                cmd(
                    link,
                    0xE0,
                    &[
                        0x0F, 0x1A, 0x0F, 0x18, 0x2F, 0x28, 0x20, 0x22, 0x1F, 0x1B, 0x23, 0x37,
                        0x00, 0x07, 0x02, 0x10,
                    ],
                )?;
                cmd(
                    link,
                    0xE1,
                    &[
                        0x0F, 0x1B, 0x0F, 0x17, 0x33, 0x2C, 0x29, 0x2E, 0x30, 0x30, 0x39, 0x3F,
                        0x00, 0x07, 0x03, 0x10,
                    ],
                )?;
            }
            Controller::Ili9341 => {
                cmd(link, 0xEF, &[0x03, 0x80, 0x02])?;
                cmd(link, 0xCF, &[0x00, 0xC1, 0x30])?; // power control B
                cmd(link, 0xED, &[0x64, 0x03, 0x12, 0x81])?; // power on sequence
                cmd(link, 0xE8, &[0x85, 0x00, 0x78])?; // driver timing A
                cmd(link, 0xCB, &[0x39, 0x2C, 0x00, 0x34, 0x02])?; // power control A
                cmd(link, 0xF7, &[0x20])?; // pump ratio
                cmd(link, 0xEA, &[0x00, 0x00])?; // driver timing B
                cmd(link, 0xC0, &[0x23])?; // power control 1
                cmd(link, 0xC1, &[0x10])?; // power control 2
                cmd(link, 0xC5, &[0x3E, 0x28])?; // VCOM 1
                cmd(link, 0xC7, &[0x86])?; // VCOM 2
                cmd(link, COLMOD, &[0x55])?; // 16 bits a pixel
                cmd(link, 0xB1, &[0x00, 0x18])?; // frame rate
                cmd(link, 0xB6, &[0x08, 0x82, 0x27])?; // display function
                cmd(link, 0xF2, &[0x00])?; // 3-gamma off
                cmd(link, 0x26, &[0x01])?; // gamma curve 1
                cmd(
                    link,
                    0xE0,
                    &[
                        0x0F, 0x31, 0x2B, 0x0C, 0x0E, 0x08, 0x4E, 0xF1, 0x37, 0x07, 0x10, 0x03,
                        0x0E, 0x09, 0x00,
                    ],
                )?;
                cmd(
                    link,
                    0xE1,
                    &[
                        0x00, 0x0E, 0x14, 0x03, 0x11, 0x07, 0x31, 0xC1, 0x48, 0x08, 0x0F, 0x0C,
                        0x31, 0x36, 0x0F,
                    ],
                )?;
            }
            _ => return Err(not_driven(entry.controller)),
        }
        cmd(link, MADCTL, &[madctl])?;
        cmd(link, if g.invert { INVON } else { INVOFF }, &[])?;
        cmd(link, NORON, &[])
    }

    /// Rows `rows` of the frame into the glass's window.
    pub fn show_rows(
        link: &Link,
        entry: &Entry,
        rotate: Rotate,
        canvas: &Canvas,
        rows: std::ops::Range<u16>,
    ) -> io::Result<()> {
        debug_assert_eq!(entry.format(), Format::Rgb565);
        let (_, x0, y0) = placement(entry, rotate);
        let w = canvas.width();
        let [xa, xb] = x0.to_be_bytes();
        let [xc, xd] = (x0 + w - 1).to_be_bytes();
        let [ya, yb] = (y0 + rows.start).to_be_bytes();
        let [yc, yd] = (y0 + rows.end - 1).to_be_bytes();
        cmd(link, CASET, &[xa, xb, xc, xd])?;
        cmd(link, RASET, &[ya, yb, yc, yd])?;
        link.command(&[RAMWR])?;
        let row_bytes = w as usize * 2;
        link.data(&canvas.bytes()[rows.start as usize * row_bytes..rows.end as usize * row_bytes])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog;

    #[test]
    fn a_panel_refuses_a_bus_it_does_not_have() {
        let entry = catalog::find("ssd1351_128x128").expect("catalogued");
        let err = Oled::open(
            entry,
            &Wiring::I2c {
                bus: 1,
                addr: 0x3C,
                reset: None,
            },
        )
        .expect_err("SPI only");
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn an_undriven_controller_says_so_before_touching_the_bus() {
        let entry = catalog::find("ch1120_128x128").expect("catalogued");
        assert!(!driven(entry.controller));
        let err = Oled::open(
            entry,
            &Wiring::I2c {
                bus: 200,
                addr: 0x3C,
                reset: None,
            },
        )
        .expect_err("not driven");
        assert_eq!(err.kind(), io::ErrorKind::Unsupported);
        // A driven controller on a glass the canvas cannot yet turn for.
        let sideways = catalog::find("sh1107_128x64").expect("catalogued");
        assert!(driven(sideways.controller) && !driven_entry(sideways));
        let err = Oled::open(
            sideways,
            &Wiring::I2c {
                bus: 200,
                addr: 0x3C,
                reset: None,
            },
        )
        .expect_err("not at this size");
        assert!(err.to_string().contains("at this size"), "{err}");
    }

    #[test]
    fn the_one_bit_family_is_driven_with_its_glass_geometry() {
        use paged::geometry;
        for id in [
            "ssd1306_128x64",
            "ssd1306_128x32",
            "ssd1315_128x64",
            "ssd1309_128x64",
            "ssd1305_128x32",
            "sh1106_128x64",
            "sh1107_64x128",
            "sh1107_128x128",
        ] {
            let e = catalog::find(id).expect(id);
            assert!(driven_entry(e), "{id}");
            assert_eq!(e.format(), Format::Paged1, "{id}");
        }
        let g = geometry(catalog::find("ssd1306_128x64").unwrap());
        assert_eq!(
            (g.multiplex, g.row_offset, g.col_offset, g.com_pins),
            (0x3F, 0, 0, 0x12)
        );
        let g = geometry(catalog::find("ssd1306_128x32").unwrap());
        assert_eq!((g.multiplex, g.com_pins), (0x1F, 0x02));
        let g = geometry(catalog::find("ssd1306_64x48").unwrap());
        assert_eq!((g.multiplex, g.col_offset), (0x2F, 32));
        let g = geometry(catalog::find("sh1106_128x64").unwrap());
        assert_eq!(g.col_offset, 2);
        let g = geometry(catalog::find("sh1107_64x128").unwrap());
        assert_eq!((g.multiplex, g.row_offset), (0x7F, 0x60));
        let g = geometry(catalog::find("ssd1305_128x32").unwrap());
        assert_eq!((g.multiplex, g.row_offset, g.col_offset), (0x3F, 32, 4));
        // The page write addresses sixteen pages of 64 bytes on the tall glass.
        let c = Canvas::new(Format::Paged1, 64, 128);
        assert_eq!((c.pages(), c.page(15).len()), (16, 64));
    }

    #[test]
    fn the_grey_family_knows_its_column_units() {
        use grey::geometry;
        for id in [
            "ssd1327_128x128",
            "ssd1327_128x96",
            "ssd1327_96x96",
            "ssd1322_256x64",
            "ssd1325_128x64",
            "sh1122_256x64",
            "ssd1362_256x64",
        ] {
            let e = catalog::find(id).expect(id);
            assert!(driven_entry(e), "{id}");
            assert_eq!(e.format(), Format::Grey4, "{id}");
        }
        // SSD1322: four pixels an address, the 256 glass centred in 480 columns.
        let g = geometry(catalog::find("ssd1322_256x64").unwrap());
        assert_eq!((g.col_start, g.col_end, g.multiplex), (0x1C, 0x5B, 0x3F));
        // SSD1327: two pixels an address.
        let g = geometry(catalog::find("ssd1327_128x128").unwrap());
        assert_eq!(
            (g.col_start, g.col_end, g.multiplex, g.row_offset),
            (0, 63, 0x7F, 0)
        );
        let g = geometry(catalog::find("ssd1327_128x96").unwrap());
        assert_eq!((g.multiplex, g.row_offset), (0x5F, 0x20));
        let g = geometry(catalog::find("ssd1327_96x96").unwrap());
        assert_eq!((g.col_start, g.col_end), (16, 63));
        let g = geometry(catalog::find("ssd1325_128x64").unwrap());
        assert_eq!((g.row_offset, g.col_end), (0x4C, 63));
        let g = geometry(catalog::find("sh1122_256x64").unwrap());
        assert_eq!((g.col_start, g.col_end), (0, 127));
        // A 256x64 grey frame is 64 rows of 128 bytes, as the SH1122 row write takes it.
        let c = Canvas::new(Format::Grey4, 256, 64);
        assert_eq!(c.bytes().len(), 64 * 128);
    }

    #[test]
    fn a_tft_glass_keeps_its_place_in_the_ram_through_every_turn() {
        use tft::placement;
        // The 2.0" fills its RAM: every turn starts at the origin, the
        // sides swap on the quarter turns.
        let e = catalog::find("st7789_240x320").unwrap();
        assert_eq!(placement(e, Rotate::Normal), (0x00, 0, 0));
        assert_eq!(placement(e, Rotate::Half), (0xC0, 0, 0));
        assert_eq!(placement(e, Rotate::Quarter), (0x60, 0, 0));
        assert_eq!(placement(e, Rotate::ThreeQuarter), (0xA0, 0, 0));
        // The square 1.3" sits in the first 240 rows, so upside down it
        // starts 80 rows in.
        let e = catalog::find("st7789_240x240").unwrap();
        assert_eq!(placement(e, Rotate::Normal), (0x00, 0, 0));
        assert_eq!(placement(e, Rotate::Half), (0xC0, 0, 80));
        assert_eq!(placement(e, Rotate::Quarter), (0x60, 0, 0));
        assert_eq!(placement(e, Rotate::ThreeQuarter), (0xA0, 80, 0));
        // The 1.14" is 52 columns and 40 rows in; its far corners follow.
        let e = catalog::find("st7789_135x240").unwrap();
        assert_eq!(placement(e, Rotate::Normal), (0x00, 52, 40));
        assert_eq!(placement(e, Rotate::Half), (0xC0, 53, 40));
        assert_eq!(placement(e, Rotate::Quarter), (0x60, 40, 53));
        assert_eq!(placement(e, Rotate::ThreeQuarter), (0xA0, 40, 52));
        // The BGR glasses carry the colour bit; the ILI9341 scans X the
        // other way, so its four turns are Adafruit's 0x48, 0x28, 0x88, 0xE8.
        let e = catalog::find("ili9341_240x320").unwrap();
        assert_eq!(placement(e, Rotate::Normal), (0x48, 0, 0));
        assert_eq!(placement(e, Rotate::Quarter), (0x28, 0, 0));
        assert_eq!(placement(e, Rotate::Half), (0x88, 0, 0));
        assert_eq!(placement(e, Rotate::ThreeQuarter), (0xE8, 0, 0));
        assert!(e.controller.is_tft() && e.format() == Format::Rgb565);
        assert_eq!(spi_settings(Controller::St7789).1, 40_000_000);
        assert_eq!(spi_settings(Controller::St7735).1, 15_000_000);
    }

    #[test]
    fn the_wiring_knows_its_bus() {
        assert_eq!(
            Wiring::Spi {
                bus: 0,
                cs: 0,
                dc: 25,
                reset: Some(27),
                backlight: None,
            }
            .bus(),
            Bus::Spi
        );
        assert_eq!(
            Wiring::I2c {
                bus: 1,
                addr: 0x3C,
                reset: None
            }
            .bus(),
            Bus::I2c
        );
        assert_eq!(spi_settings(Controller::Ssd1351), (0, 10_000_000));
    }
}
