//! The daemon's settings: the plugin's configuration store read into what
//! the driver layer needs. The plugin writes a preset's pins over the
//! stored ones when a connection is saved, so one set of keys is read.

use std::fmt;

use justoled::canvas::Rotate;
use justoled::catalog::{self, Entry};
use justoled::json::Store;
use justoled::oled::Wiring;

/// Where Volumio keeps the plugin's store.
pub const DEFAULT_PATH: &str = "/data/configuration/system_hardware/just_oled/config.json";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    pub entry: &'static Entry,
    pub wiring: Wiring,
    pub rotate: Rotate,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    Json(String),
    /// A key missing or of the wrong shape.
    Field(&'static str, String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Json(e) => write!(f, "the settings are not JSON: {e}"),
            Error::Field(key, what) => write!(f, "setting {key}: {what}"),
        }
    }
}

impl Settings {
    pub fn parse(text: &str) -> Result<Settings, Error> {
        let store = Store::parse(text).map_err(|e| Error::Json(e.to_string()))?;
        Settings::from_store(&store)
    }

    pub fn from_store(store: &Store) -> Result<Settings, Error> {
        let panel = store
            .string("panel")
            .ok_or_else(|| Error::Field("panel", "missing".into()))?;
        let entry = catalog::find(&panel)
            .ok_or_else(|| Error::Field("panel", format!("{panel} is not in the catalogue")))?;
        let connection = store.string("connection").unwrap_or_else(|| "i2c".into());
        let pin = |key: &'static str| -> Result<Option<u32>, Error> {
            match store.number(key) {
                None => Ok(None),
                Some(n) if (0.0..=27.0).contains(&n) && n.fract() == 0.0 => Ok(Some(n as u32)),
                Some(n) => Err(Error::Field(key, format!("{n} is not a GPIO number"))),
            }
        };
        let small = |key: &'static str, default: u8| -> Result<u8, Error> {
            match store.number(key) {
                None => Ok(default),
                Some(n) if (0.0..=255.0).contains(&n) && n.fract() == 0.0 => Ok(n as u8),
                Some(n) => Err(Error::Field(key, format!("{n} is out of range"))),
            }
        };
        let wiring = if connection.starts_with("spi") {
            Wiring::Spi {
                bus: small("spi_bus", 0)?,
                cs: small("spi_cs", 0)?,
                dc: pin("gpio_dc")?.ok_or_else(|| Error::Field("gpio_dc", "missing".into()))?,
                reset: pin("gpio_reset")?,
            }
        } else {
            let addr = match store.string("i2c_address").as_deref() {
                None | Some("auto") | Some("") => {
                    *entry.i2c_addresses.first().ok_or_else(|| {
                        Error::Field("i2c_address", format!("{} has no I2C address", entry.id))
                    })?
                }
                Some(text) => parse_address(text).ok_or_else(|| {
                    Error::Field("i2c_address", format!("{text} is not an address"))
                })?,
            };
            Wiring::I2c {
                bus: small("i2c_bus", 1)?,
                addr,
                // The settings page has no reset pin for I2C modules; most tie it to VCC.
                reset: None,
            }
        };
        let rotate = Rotate::from_degrees(store.number("rotate").unwrap_or(0.0) as i64);
        Ok(Settings {
            entry,
            wiring,
            rotate,
        })
    }
}

/// `0x3C`, `3C` or `60`.
fn parse_address(text: &str) -> Option<u8> {
    let t = text.trim();
    let hex = t
        .strip_prefix("0x")
        .or_else(|| t.strip_prefix("0X"))
        .map(|h| u8::from_str_radix(h, 16).ok());
    match hex {
        Some(v) => v,
        None => t
            .parse::<u8>()
            .ok()
            .or_else(|| u8::from_str_radix(t, 16).ok()),
    }
    .filter(|a| (0x08..=0x77).contains(a))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(pairs: &[(&str, &str)]) -> String {
        let mut s = String::from("{");
        for (i, (k, v)) in pairs.iter().enumerate() {
            if i > 0 {
                s.push(',');
            }
            s.push_str(&format!("\"{k}\":{{\"type\":\"x\",\"value\":{v}}}"));
        }
        s.push('}');
        s
    }

    #[test]
    fn the_waveshare_preset_reads_back_as_spi_wiring() {
        let text = store(&[
            ("connection", "\"spi_waveshare\""),
            ("panel", "\"ssd1351_128x128\""),
            ("spi_bus", "0"),
            ("spi_cs", "0"),
            ("gpio_dc", "25"),
            ("gpio_reset", "27"),
            ("rotate", "\"180\""),
        ]);
        let s = Settings::parse(&text).expect("parses");
        assert_eq!(s.entry.id, "ssd1351_128x128");
        assert_eq!(
            s.wiring,
            Wiring::Spi {
                bus: 0,
                cs: 0,
                dc: 25,
                reset: Some(27)
            }
        );
        assert_eq!(s.rotate, Rotate::Half);
    }

    #[test]
    fn i2c_takes_the_panel_s_first_address_when_auto() {
        let text = store(&[
            ("connection", "\"i2c\""),
            ("panel", "\"ssd1306_128x64\""),
            ("i2c_bus", "1"),
            ("i2c_address", "\"auto\""),
            ("gpio_reset", "25"),
        ]);
        let s = Settings::parse(&text).expect("parses");
        assert_eq!(
            s.wiring,
            Wiring::I2c {
                bus: 1,
                addr: 0x3C,
                reset: None
            }
        );
        assert_eq!(s.rotate, Rotate::Normal);
        let text = store(&[
            ("connection", "\"i2c_custom\""),
            ("panel", "\"ssd1306_128x64\""),
            ("i2c_bus", "3"),
            ("i2c_address", "\"0x3D\""),
        ]);
        let s = Settings::parse(&text).expect("parses");
        assert_eq!(
            s.wiring,
            Wiring::I2c {
                bus: 3,
                addr: 0x3D,
                reset: None
            }
        );
    }

    #[test]
    fn bad_values_name_their_key() {
        let text = store(&[("connection", "\"spi\""), ("panel", "\"nope\"")]);
        assert_eq!(
            Settings::parse(&text),
            Err(Error::Field("panel", "nope is not in the catalogue".into()))
        );
        let text = store(&[
            ("connection", "\"spi\""),
            ("panel", "\"ssd1351_128x128\""),
            ("gpio_dc", "99"),
        ]);
        assert!(matches!(
            Settings::parse(&text),
            Err(Error::Field("gpio_dc", _))
        ));
        assert!(matches!(Settings::parse("nonsense"), Err(Error::Json(_))));
        assert_eq!(parse_address("0x3c"), Some(0x3C));
        assert_eq!(parse_address("3D"), Some(0x3D));
        assert_eq!(parse_address("60"), Some(60));
        assert_eq!(parse_address("0x00"), None);
    }
}
