//! The catalogue: every controller and panel geometry the daemon knows,
//! one entry each, named by a stable id the settings page stores. An entry
//! says what the panel is and how it is reached; the bytes that start it
//! and the way its frame is written belong to the driver layer, keyed by
//! the same id.
//!
//! `verified` tells a user what stands behind an entry: `Bench` has drawn
//! on a module in hand; `Described` is transcribed from the controller's
//! data sheet and reference drivers and awaits a module.

use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Controller {
    Ssd1305,
    Ssd1306,
    Ssd1309,
    /// SSD1306 command set on a 3.3 V panel without a charge pump.
    Ssd1315,
    Ssd1322,
    Ssd1325,
    Ssd1327,
    Ssd1331,
    Ssd1351,
    Ssd1357,
    Ssd1362,
    Sh1106,
    Sh1107,
    Sh1122,
    Ch1120,
    /// Sitronix TFT LCD controllers, 240x320 RAM, RGB565 over four-wire SPI.
    St7789,
    /// Sitronix TFT LCD controller, 132x162 RAM.
    St7735,
    /// ILI9341 TFT LCD controller, 240x320 RAM.
    Ili9341,
}

impl Controller {
    pub fn name(self) -> &'static str {
        match self {
            Controller::Ssd1305 => "SSD1305",
            Controller::Ssd1306 => "SSD1306",
            Controller::Ssd1309 => "SSD1309",
            Controller::Ssd1315 => "SSD1315",
            Controller::Ssd1322 => "SSD1322",
            Controller::Ssd1325 => "SSD1325",
            Controller::Ssd1327 => "SSD1327",
            Controller::Ssd1331 => "SSD1331",
            Controller::Ssd1351 => "SSD1351",
            Controller::Ssd1357 => "SSD1357",
            Controller::Ssd1362 => "SSD1362",
            Controller::Sh1106 => "SH1106",
            Controller::Sh1107 => "SH1107",
            Controller::Sh1122 => "SH1122",
            Controller::Ch1120 => "CH1120",
            Controller::St7789 => "ST7789",
            Controller::St7735 => "ST7735",
            Controller::Ili9341 => "ILI9341",
        }
    }

    /// The pixel format the controller's RAM holds.
    pub fn format(self) -> Format {
        match self {
            Controller::Ssd1305
            | Controller::Ssd1306
            | Controller::Ssd1309
            | Controller::Ssd1315
            | Controller::Sh1106
            | Controller::Sh1107
            | Controller::Ch1120 => Format::Paged1,
            Controller::Ssd1322
            | Controller::Ssd1325
            | Controller::Ssd1327
            | Controller::Ssd1362
            | Controller::Sh1122 => Format::Grey4,
            Controller::Ssd1331
            | Controller::Ssd1351
            | Controller::Ssd1357
            | Controller::St7789
            | Controller::St7735
            | Controller::Ili9341 => Format::Rgb565,
        }
    }

    /// Whether the controller is a TFT LCD rather than an OLED: it has a
    /// backlight, and turns the picture a quarter turn itself.
    pub fn is_tft(self) -> bool {
        matches!(
            self,
            Controller::St7789 | Controller::St7735 | Controller::Ili9341
        )
    }
}

impl fmt::Display for Controller {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// How pixels sit in the controller's RAM, and so how a frame is sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// One bit per pixel, a byte being eight pixels down a column, in pages of eight rows.
    Paged1,
    /// Four bits per pixel, two pixels side by side in a byte, rows in order.
    Grey4,
    /// Sixteen bits per pixel, red 5, green 6, blue 5, rows in order.
    Rgb565,
}

impl Format {
    pub fn name(self) -> &'static str {
        match self {
            Format::Paged1 => "1 bpp",
            Format::Grey4 => "4 bpp grey",
            Format::Rgb565 => "RGB565",
        }
    }

    /// Bytes one frame of `width` by `height` takes.
    pub fn frame_bytes(self, width: u16, height: u16) -> usize {
        let (w, h) = (width as usize, height as usize);
        match self {
            Format::Paged1 => w * h.div_ceil(8),
            Format::Grey4 => w.div_ceil(2) * h,
            Format::Rgb565 => w * h * 2,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bus {
    I2c,
    /// Four-wire SPI: clock, data, chip select, data/command, with a reset line.
    Spi,
}

impl Bus {
    pub fn name(self) -> &'static str {
        match self {
            Bus::I2c => "I2C",
            Bus::Spi => "SPI",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verified {
    /// Drawn on a module in hand.
    Bench,
    /// Transcribed from the data sheet and reference drivers; no module yet.
    Described,
}

impl Verified {
    pub fn name(self) -> &'static str {
        match self {
            Verified::Bench => "verified",
            Verified::Described => "described",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry {
    /// Stable, lowercase, what the settings store: `ssd1306_128x64`.
    pub id: &'static str,
    pub controller: Controller,
    pub width: u16,
    pub height: u16,
    pub buses: &'static [Bus],
    /// The seven-bit addresses the panel answers on, where it has I2C.
    pub i2c_addresses: &'static [u8],
    pub verified: Verified,
    /// Modules known to carry this panel, and what to watch for.
    pub note: &'static str,
}

impl Entry {
    pub fn format(&self) -> Format {
        self.controller.format()
    }

    pub fn has_bus(&self, bus: Bus) -> bool {
        self.buses.contains(&bus)
    }

    pub fn frame_bytes(&self) -> usize {
        self.format().frame_bytes(self.width, self.height)
    }
}

const I2C: &[Bus] = &[Bus::I2c];
const SPI: &[Bus] = &[Bus::Spi];
const BOTH: &[Bus] = &[Bus::I2c, Bus::Spi];
const A3C: &[u8] = &[0x3C];
const A3C3D: &[u8] = &[0x3C, 0x3D];
const NONE: &[u8] = &[];

/// Every panel, in the order the settings page lists them.
pub const ENTRIES: &[Entry] = &[
    Entry {
        id: "ssd1306_128x64",
        controller: Controller::Ssd1306,
        width: 128,
        height: 64,
        buses: BOTH,
        i2c_addresses: A3C3D,
        verified: Verified::Bench,
        note: "0.96\" and 1.3\" modules: Waveshare 0.96inch OLED (A)/(B), Adafruit 326 and 938, DFRobot DFR0486 and DFR0650, the generic four-pin I2C board; also the panel in most Pi cases (Argon, Pironman). Verified on a generic 0.96\" module over I2C at 0x3C.",
    },
    Entry {
        id: "ssd1306_128x32",
        controller: Controller::Ssd1306,
        width: 128,
        height: 32,
        buses: BOTH,
        i2c_addresses: A3C3D,
        verified: Verified::Bench,
        note: "0.91\" modules: Waveshare 0.91inch OLED Module, Adafruit 4440 and 661, SparkFun Qwiic 0.91\", DFRobot DFR0647/0648, 52Pi and UCTRONICS rack panels. Verified on a generic 0.91\" module over I2C at 0x3C.",
    },
    Entry {
        id: "ssd1306_64x48",
        controller: Controller::Ssd1306,
        width: 64,
        height: 48,
        buses: BOTH,
        i2c_addresses: A3C3D,
        verified: Verified::Described,
        note: "0.66\" modules: SparkFun Micro OLED, Grove OLED 0.66\", the Wemos shield. A window of the 128x64 RAM, columns 32 to 95 and rows 16 to 63.",
    },
    Entry {
        id: "ssd1306_72x40",
        controller: Controller::Ssd1306,
        width: 72,
        height: 40,
        buses: I2C,
        i2c_addresses: A3C,
        verified: Verified::Described,
        note: "0.42\" modules. A window of the 128x64 RAM with a column offset; the offset is proven on a module, not assumed.",
    },
    Entry {
        id: "ssd1306_64x32",
        controller: Controller::Ssd1306,
        width: 64,
        height: 32,
        buses: I2C,
        i2c_addresses: A3C,
        verified: Verified::Described,
        note: "0.49\" modules such as Waveshare's (SSD1315): the first half of the 128x64 buffer.",
    },
    Entry {
        id: "ssd1306_96x16",
        controller: Controller::Ssd1306,
        width: 96,
        height: 16,
        buses: I2C,
        i2c_addresses: A3C,
        verified: Verified::Described,
        note: "0.69\" and 0.71\" modules.",
    },
    Entry {
        id: "ssd1315_128x64",
        controller: Controller::Ssd1315,
        width: 128,
        height: 64,
        buses: BOTH,
        i2c_addresses: A3C3D,
        verified: Verified::Described,
        note: "Waveshare 0.96inch OLED Module (C)/(D)/(E), Grove OLED 0.96\" (SSD1315) and Yellow&Blue, Waveshare OLED/LCD HAT (A). Driven with the SSD1306 command set.",
    },
    Entry {
        id: "ssd1309_128x64",
        controller: Controller::Ssd1309,
        width: 128,
        height: 64,
        buses: BOTH,
        i2c_addresses: A3C3D,
        verified: Verified::Bench,
        note: "2.42\" and 1.54\" modules and the transparent 1.51\": Waveshare 2.42inch OLED Module, 1.54inch OLED Module, 1.51inch Transparent OLED, DFRobot DFR0934, HiLetgo 2.42\". No charge pump; the reset line must be driven. Verified on a 2.42\" module over SPI and over I2C at 0x3C.",
    },
    Entry {
        id: "ssd1305_128x32",
        controller: Controller::Ssd1305,
        width: 128,
        height: 32,
        buses: BOTH,
        i2c_addresses: A3C3D,
        verified: Verified::Described,
        note: "2.23\" modules: Waveshare 2.23inch OLED HAT and Pico-OLED-2.23, Adafruit 2675. The RAM is 132 columns wide; the column offset is per glass.",
    },
    Entry {
        id: "sh1106_128x64",
        controller: Controller::Sh1106,
        width: 128,
        height: 64,
        buses: BOTH,
        i2c_addresses: A3C3D,
        verified: Verified::Bench,
        note: "1.3\" modules: Waveshare 1.3inch OLED (A)/(B) and 1.3inch OLED HAT, the generic 1.3\" four-pin I2C board. Written a page at a time; two columns of offset in the 132-wide RAM. Verified on a generic 1.3\" module over I2C at 0x3C.",
    },
    Entry {
        id: "sh1107_128x128",
        controller: Controller::Sh1107,
        width: 128,
        height: 128,
        buses: BOTH,
        i2c_addresses: A3C3D,
        verified: Verified::Described,
        note: "1.12\" and 1.5\" modules: Adafruit 5297, Pimoroni 1.12\", Grove OLED 1.12\" V2/V3, Waveshare 1.5inch OLED Module (B). Pages run along the long axis.",
    },
    Entry {
        id: "sh1107_64x128",
        controller: Controller::Sh1107,
        width: 64,
        height: 128,
        buses: BOTH,
        i2c_addresses: A3C3D,
        verified: Verified::Described,
        note: "Waveshare 1.3inch OLED Module (C) and Pico-OLED-1.3. Half of the 128x128 RAM; some want SPI mode 3.",
    },
    Entry {
        id: "sh1107_128x64",
        controller: Controller::Sh1107,
        width: 128,
        height: 64,
        buses: I2C,
        i2c_addresses: A3C3D,
        verified: Verified::Described,
        note: "Adafruit 128x64 OLED FeatherWing (4650).",
    },
    Entry {
        id: "ssd1327_128x128",
        controller: Controller::Ssd1327,
        width: 128,
        height: 128,
        buses: BOTH,
        i2c_addresses: A3C3D,
        verified: Verified::Described,
        note: "1.5\" greyscale modules: Waveshare 1.5inch OLED Module, Adafruit 4741. Sixteen grey levels.",
    },
    Entry {
        id: "ssd1327_128x96",
        controller: Controller::Ssd1327,
        width: 128,
        height: 96,
        buses: BOTH,
        i2c_addresses: A3C3D,
        verified: Verified::Described,
        note: "Waveshare 1.32inch OLED Module.",
    },
    Entry {
        id: "ssd1327_96x96",
        controller: Controller::Ssd1327,
        width: 96,
        height: 96,
        buses: I2C,
        i2c_addresses: A3C3D,
        verified: Verified::Described,
        note: "Grove OLED 1.12\" V1.0.",
    },
    Entry {
        id: "ssd1322_256x64",
        controller: Controller::Ssd1322,
        width: 256,
        height: 64,
        buses: SPI,
        i2c_addresses: NONE,
        verified: Verified::Described,
        note: "2.08\", 2.7\", 2.8\" and 3.12\" 256x64 greyscale modules: Newhaven NHD-3.12-25664 and NHD-2.8-25664, the EastRising ER-OLEDM032 family, the panel mpd_oled builds favour. Column addresses count four pixels; the 256-wide glass starts at column 0x1C.",
    },
    Entry {
        id: "ssd1325_128x64",
        controller: Controller::Ssd1325,
        width: 128,
        height: 64,
        buses: SPI,
        i2c_addresses: NONE,
        verified: Verified::Described,
        note: "Adafruit 2.7\" 128x64 module kit (2674), Newhaven NHD-2.7-12864.",
    },
    Entry {
        id: "sh1122_256x64",
        controller: Controller::Sh1122,
        width: 256,
        height: 64,
        buses: BOTH,
        i2c_addresses: A3C3D,
        verified: Verified::Described,
        note: "2.08\" 256x64 greyscale modules.",
    },
    Entry {
        id: "ssd1362_256x64",
        controller: Controller::Ssd1362,
        width: 256,
        height: 64,
        buses: BOTH,
        i2c_addresses: A3C3D,
        verified: Verified::Described,
        note: "3.12\" 256x64 greyscale modules of the newer kind.",
    },
    Entry {
        id: "ch1120_128x128",
        controller: Controller::Ch1120,
        width: 128,
        height: 128,
        buses: BOTH,
        i2c_addresses: A3C3D,
        verified: Verified::Described,
        note: "SparkFun Qwiic OLED 1.5\" 128x128 (LCD-29530), Winstar WEO128128HWAP3N00000.",
    },
    Entry {
        id: "ssd1331_96x64",
        controller: Controller::Ssd1331,
        width: 96,
        height: 64,
        buses: SPI,
        i2c_addresses: NONE,
        verified: Verified::Bench,
        note: "0.95\" and 0.96\" colour modules: Waveshare 0.95inch RGB OLED (A)/(B), Adafruit 684. Verified on a generic 0.95\" module over SPI.",
    },
    Entry {
        id: "ssd1351_128x128",
        controller: Controller::Ssd1351,
        width: 128,
        height: 128,
        buses: SPI,
        i2c_addresses: NONE,
        verified: Verified::Bench,
        note: "1.5\" colour modules: Waveshare 1.5inch RGB OLED Module (on the bench), Adafruit 1431, Newhaven NHD-1.5-128128UGC3.",
    },
    Entry {
        id: "ssd1351_128x96",
        controller: Controller::Ssd1351,
        width: 128,
        height: 96,
        buses: SPI,
        i2c_addresses: NONE,
        verified: Verified::Described,
        note: "1.27\" colour modules: Waveshare 1.27inch RGB OLED Module, Adafruit 1673, Newhaven NHD-1.27-12896UGC3. Rows 0 to 95 of the 128x128 RAM.",
    },
    Entry {
        id: "ssd1357_64x128",
        controller: Controller::Ssd1357,
        width: 64,
        height: 128,
        buses: SPI,
        i2c_addresses: NONE,
        verified: Verified::Described,
        note: "Waveshare 0.96inch RGB OLED Module. A 64-column window of the 128x128 RAM.",
    },
    // The TFT LCDs: a backlight pin besides the OLED's seven wires, and a
    // quarter turn the controller makes itself. Sizes are the glass in its
    // own portrait; the Orientation setting turns it.
    Entry {
        id: "st7789_240x320",
        controller: Controller::St7789,
        width: 240,
        height: 320,
        buses: SPI,
        i2c_addresses: NONE,
        verified: Verified::Bench,
        note: "2.0\" IPS modules: Waveshare 2inch LCD Module (ST7789V, seen on the bench at every turn), Adafruit 4311, Pimoroni Display HAT Mini (CE1, DC 9, backlight 13). The whole 240x320 RAM.",
    },
    Entry {
        id: "st7789_240x240",
        controller: Controller::St7789,
        width: 240,
        height: 240,
        buses: SPI,
        i2c_addresses: NONE,
        verified: Verified::Described,
        note: "1.3\" and 1.54\" square IPS modules: Waveshare 1.3inch LCD Module, Pimoroni Pirate Audio HATs (CE1, DC 9, backlight 13), Adafruit 4313 and 3787. The first 240 rows of the RAM.",
    },
    Entry {
        id: "st7789_135x240",
        controller: Controller::St7789,
        width: 135,
        height: 240,
        buses: SPI,
        i2c_addresses: NONE,
        verified: Verified::Described,
        note: "1.14\" IPS modules: Waveshare 1.14inch LCD Module, Adafruit 4383. Columns 52 to 186 and rows 40 to 279 of the RAM.",
    },
    Entry {
        id: "st7789_170x320",
        controller: Controller::St7789,
        width: 170,
        height: 320,
        buses: SPI,
        i2c_addresses: NONE,
        verified: Verified::Described,
        note: "1.9\" IPS modules: Waveshare 1.9inch LCD Module, Adafruit 5394. Columns 35 to 204 of the RAM.",
    },
    Entry {
        id: "st7735_128x160",
        controller: Controller::St7735,
        width: 128,
        height: 160,
        buses: SPI,
        i2c_addresses: NONE,
        verified: Verified::Described,
        note: "1.8\" TFT modules: Waveshare 1.8inch LCD Module, Adafruit 358, the common red-tab boards. Colour order differs between boards; a swapped red and blue means the other order.",
    },
    Entry {
        id: "st7735_80x160",
        controller: Controller::St7735,
        width: 80,
        height: 160,
        buses: SPI,
        i2c_addresses: NONE,
        verified: Verified::Described,
        note: "0.96\" IPS modules: Waveshare 0.96inch LCD Module, Adafruit 3533. Columns 26 to 105 of the RAM, picture inverted as these glasses need.",
    },
    Entry {
        id: "ili9341_240x320",
        controller: Controller::Ili9341,
        width: 240,
        height: 320,
        buses: SPI,
        i2c_addresses: NONE,
        verified: Verified::Bench,
        note: "2.2\" to 3.2\" TFT modules: Waveshare 2.4inch LCD Module, Adafruit 1770 and 1743, the common 2.8\" boards with a touch panel. The whole 240x320 RAM, scanned the other way round from the ST7789. Verified on a bare 2.8\" board (GND VCC CLK MOSI RES DC BLK MISO, no CS; BLK to 3.3 V).",
    },
];

/// The entry with this id, if any.
pub fn find(id: &str) -> Option<&'static Entry> {
    ENTRIES.iter().find(|e| e.id == id)
}

/// The entries that can be reached over `bus`, in catalogue order.
pub fn on_bus(bus: Bus) -> impl Iterator<Item = &'static Entry> {
    ENTRIES.iter().filter(move |e| e.has_bus(bus))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn ids_are_unique_lowercase_and_match_the_geometry() {
        let mut seen = HashSet::new();
        for e in ENTRIES {
            assert!(seen.insert(e.id), "{} twice", e.id);
            assert!(
                e.id.bytes().all(|b| b.is_ascii_lowercase()
                    || b.is_ascii_digit()
                    || b == b'_'
                    || b == b'x'),
                "{} has a character the settings store should not see",
                e.id
            );
            let expected = format!(
                "{}_{}x{}",
                e.controller.name().to_ascii_lowercase(),
                e.width,
                e.height
            );
            assert_eq!(e.id, expected, "id and geometry disagree");
        }
    }

    #[test]
    fn every_entry_is_reachable_and_i2c_entries_have_addresses() {
        for e in ENTRIES {
            assert!(!e.buses.is_empty(), "{} has no bus", e.id);
            assert_eq!(
                e.has_bus(Bus::I2c),
                !e.i2c_addresses.is_empty(),
                "{}: I2C and addresses disagree",
                e.id
            );
            for &a in e.i2c_addresses {
                assert!(a < 0x80, "{}: {a:#x} is not a seven-bit address", e.id);
            }
        }
    }

    #[test]
    fn frame_sizes_follow_the_format() {
        assert_eq!(Format::Paged1.frame_bytes(128, 64), 1024);
        assert_eq!(Format::Paged1.frame_bytes(128, 32), 512);
        assert_eq!(Format::Paged1.frame_bytes(72, 40), 72 * 5);
        assert_eq!(Format::Grey4.frame_bytes(256, 64), 8192);
        assert_eq!(Format::Grey4.frame_bytes(128, 128), 8192);
        assert_eq!(Format::Rgb565.frame_bytes(128, 128), 32768);
        assert_eq!(find("ssd1322_256x64").unwrap().frame_bytes(), 8192);
        assert_eq!(find("sh1106_128x64").unwrap().frame_bytes(), 1024);
    }

    #[test]
    fn lookups() {
        assert_eq!(
            find("ssd1306_128x64").map(|e| e.controller),
            Some(Controller::Ssd1306)
        );
        assert!(find("SSD1306_128x64").is_none());
        assert!(on_bus(Bus::Spi).all(|e| e.has_bus(Bus::Spi)));
        assert!(on_bus(Bus::I2c).any(|e| e.id == "sh1106_128x64"));
        assert!(!on_bus(Bus::I2c).any(|e| e.id == "ssd1331_96x64"));
    }
}
