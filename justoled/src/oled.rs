//! The driver layer: a panel opened on its bus, started with its
//! controller's sequence, and shown a frame. Keyed by the catalogue entry,
//! so the settings page and this file agree on what a panel is.
//!
//! Bytes go down one of two links. Four-wire SPI tells command from data
//! by the DC line; I2C by a control byte in front (0x00 command, 0x40
//! data), as the SSD1306 family and SSD1327 define it. Controllers of the
//! SSD1351 class take a command's arguments as data (DC high); the SSD1306
//! class takes them as further commands. Each sequence says which.
//!
//! Sequences are transcribed from the controller data sheets and checked
//! against the module vendor's reference drivers; the entry's `verified`
//! flag in the catalogue says whether a module has confirmed them.

use std::io;
use std::thread::sleep;
use std::time::Duration;

use crate::canvas::Canvas;
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
        /// DC at index 0; reset at index 1 when wired.
        lines: gpio::Outputs,
        has_reset: bool,
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
            Wiring::Spi { bus, cs, dc, reset } => {
                let (mode, speed) = spi_settings(entry.controller);
                let dev = spi::Device::open(bus, cs, mode, speed)
                    .map_err(|e| named(format!("/dev/spidev{bus}.{cs}"), e))?;
                let chip = chip()?;
                let lines = match reset {
                    Some(rst) => {
                        gpio::Outputs::request(&chip.path, &[dc, rst], &[false, true], CONSUMER)
                            .map_err(|e| named(format!("{} lines {dc} and {rst}", chip.path), e))?
                    }
                    None => gpio::Outputs::request(&chip.path, &[dc], &[false], CONSUMER)
                        .map_err(|e| named(format!("{} line {dc}", chip.path), e))?,
                };
                Ok(Link::Spi {
                    dev,
                    lines,
                    has_reset: reset.is_some(),
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
                has_reset: true,
                ..
            } => Some((lines, 1)),
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
        _ => (0, 8_000_000),
    }
}

/// A panel, opened and ready to be started.
pub struct Oled {
    entry: &'static Entry,
    link: Link,
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
        if !driven(entry.controller) {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                format!("{} is catalogued but not yet driven", entry.controller),
            ));
        }
        let link = Link::open(entry, wiring)?;
        Ok(Oled { entry, link })
    }

    pub fn entry(&self) -> &'static Entry {
        self.entry
    }

    /// A blank frame of the panel's size and format.
    pub fn canvas(&self) -> Canvas {
        Canvas::new(self.entry.format(), self.entry.width, self.entry.height)
    }

    /// Reset, run the controller's start-up sequence, clear the RAM and
    /// turn the display on.
    pub fn init(&mut self) -> io::Result<()> {
        self.link.reset()?;
        match self.entry.controller {
            Controller::Ssd1351 => ssd1351::init(&self.link, self.entry)?,
            _ => return Err(not_driven(self.entry.controller)),
        }
        let blank = self.canvas();
        self.show(&blank)?;
        self.on()
    }

    /// Send a whole frame. The canvas must be the panel's own.
    pub fn show(&mut self, canvas: &Canvas) -> io::Result<()> {
        if canvas.format() != self.entry.format()
            || canvas.width() != self.entry.width
            || canvas.height() != self.entry.height
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "the frame is not this panel's size",
            ));
        }
        match self.entry.controller {
            Controller::Ssd1351 => ssd1351::show(&self.link, self.entry, canvas.bytes()),
            _ => Err(not_driven(self.entry.controller)),
        }
    }

    pub fn on(&mut self) -> io::Result<()> {
        match self.entry.controller {
            Controller::Ssd1351 => self.link.command(&[ssd1351::DISPLAY_ON]),
            _ => Err(not_driven(self.entry.controller)),
        }
    }

    /// Display off: the panel goes dark and its RAM keeps the frame.
    pub fn off(&mut self) -> io::Result<()> {
        match self.entry.controller {
            Controller::Ssd1351 => self.link.command(&[ssd1351::DISPLAY_OFF]),
            _ => Err(not_driven(self.entry.controller)),
        }
    }

    /// Brightness 0..=255 as the controller counts it.
    pub fn contrast(&mut self, level: u8) -> io::Result<()> {
        match self.entry.controller {
            Controller::Ssd1351 => ssd1351::contrast(&self.link, level),
            _ => Err(not_driven(self.entry.controller)),
        }
    }
}

/// Whether a sequence exists for the controller yet.
pub fn driven(controller: Controller) -> bool {
    matches!(controller, Controller::Ssd1351)
}

fn not_driven(controller: Controller) -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        format!("{controller} is catalogued but not yet driven"),
    )
}

/// SSD1351: 128 x 128 RGB, 65K colour, in Solomon Systech's data sheet
/// rev 1.5. Command arguments go as data. Values follow the sheet's
/// reset defaults where the reference drivers (Waveshare's demo for the
/// 1.5inch RGB OLED Module, luma.oled) agree, theirs where they differ.
mod ssd1351 {
    use super::*;

    pub const DISPLAY_ON: u8 = 0xAF;
    pub const DISPLAY_OFF: u8 = 0xAE;

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
        let entry = catalog::find("ssd1306_128x64").expect("catalogued");
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
    }

    #[test]
    fn the_wiring_knows_its_bus() {
        assert_eq!(
            Wiring::Spi {
                bus: 0,
                cs: 0,
                dc: 25,
                reset: Some(27)
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
