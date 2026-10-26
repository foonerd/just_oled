//! The daemon's settings: the plugin's configuration store read into what
//! the driver layer needs. The plugin writes a preset's pins over the
//! stored ones when a connection is saved, so one set of keys is read.

use std::fmt;

use justoled::canvas::{Colour, Rotate};
use justoled::catalog::{self, Entry};
use justoled::json::Store;
use justoled::oled::Wiring;

use crate::meter::GAIN_DB_RANGE;
use crate::pages::{
    Face, Look, Theme, TimeDisplay, TrackText, CLOCK_FORMATS, DATE_FORMATS, THEMES,
};

/// Where Volumio keeps the plugin's store.
pub const DEFAULT_PATH: &str = "/data/configuration/system_hardware/just_oled/config.json";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    pub entry: &'static Entry,
    pub wiring: Wiring,
    pub rotate: Rotate,
    /// The face: what the playing and standby pages show.
    pub look: Look,
    /// Whether the boot, ready, pause and farewell texts are shown.
    pub show_lifecycle: bool,
    /// Each text as its rows, `|` splitting them.
    pub text_boot: Vec<String>,
    pub text_ready: Vec<String>,
    pub text_pause: Vec<String>,
    pub text_shutdown: Vec<String>,
    pub text_reboot: Vec<String>,
    /// The standby page: the clock, or nothing at all.
    pub idle_clock: bool,
    /// Standby time before the panel goes dark; `None` keeps the clock on.
    pub idle_timeout: Option<std::time::Duration>,
    /// How long a pause lasts before it counts as a stop; `None` never.
    pub pause_standby: Option<std::time::Duration>,
    /// `strftime` patterns, one of `CLOCK_FORMATS` and `DATE_FORMATS`.
    pub clock_format: String,
    pub date_format: String,
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
        let text = |key: &str, default: &str| -> Vec<String> {
            let t = store.string(key).unwrap_or_else(|| default.to_string());
            t.split('|').map(|s| s.trim().to_string()).collect()
        };
        let one_of = |key: &str, offered: &[&str]| -> String {
            let chosen = store.string(key).unwrap_or_default();
            if offered.contains(&chosen.as_str()) {
                chosen
            } else {
                offered[0].to_string()
            }
        };
        let look = Look {
            face: Face::from_setting(&store.string("face").unwrap_or_default()),
            text: TrackText::from_setting(&store.string("track_text").unwrap_or_default()),
            time: TimeDisplay::from_setting(&store.string("time_display").unwrap_or_default()),
            show_format: store.boolean("show_format").unwrap_or(true),
            peaks: store.boolean("peaks").unwrap_or(true),
            gain_db: store
                .number("meter_gain")
                .map_or(0, |n| n.round() as i32)
                .clamp(-GAIN_DB_RANGE, GAIN_DB_RANGE),
            show_ip: store.boolean("show_ip").unwrap_or(true),
            wide_meter: store.boolean("meter_wide").unwrap_or(false),
            theme: {
                let name = one_of("theme", THEMES);
                if name == "custom" {
                    // Each colour the picker wrote, the classic one where it did not.
                    let classic = Theme::default();
                    let colour = |key: &str, fallback: Colour| {
                        store
                            .string(key)
                            .and_then(|t| Theme::parse_colour(&t))
                            .unwrap_or(fallback)
                    };
                    Theme {
                        title: colour("colour_title", classic.title),
                        detail: colour("colour_detail", classic.detail),
                        bar_low: colour("colour_bar_low", classic.bar_low),
                        bar_high: colour("colour_bar_high", classic.bar_high),
                        peak: colour("colour_peak", classic.peak),
                        progress: colour("colour_progress", classic.progress),
                    }
                } else {
                    Theme::preset(&name)
                }
            },
        };
        Ok(Settings {
            entry,
            wiring,
            rotate,
            look,
            show_lifecycle: store.boolean("show_lifecycle").unwrap_or(true),
            text_boot: text("text_boot", "Volumio|booting..."),
            text_ready: text("text_ready", "Volumio|ready"),
            text_pause: text("text_pause", "Paused|waiting on music"),
            text_shutdown: text("text_shutdown", "Volumio|shutting down"),
            text_reboot: text("text_reboot", "Volumio|restarting"),
            idle_clock: store.string("idle_screen").as_deref() != Some("blank"),
            idle_timeout: match store.number("idle_timeout").unwrap_or(30.0) {
                m if m <= 0.0 => None,
                m => Some(std::time::Duration::from_secs(
                    (m.min(1440.0) * 60.0) as u64,
                )),
            },
            pause_standby: if store.boolean("pause_standby").unwrap_or(true) {
                Some(std::time::Duration::from_secs(
                    store
                        .number("pause_timeout")
                        .unwrap_or(0.0)
                        .clamp(0.0, 3600.0) as u64,
                ))
            } else {
                None
            },
            clock_format: one_of("clock_format", CLOCK_FORMATS),
            date_format: one_of("date_format", DATE_FORMATS),
        })
    }

    /// Whether `other` needs the panel opened anew: another panel or another wiring.
    pub fn needs_reopen(&self, other: &Settings) -> bool {
        !std::ptr::eq(self.entry, other.entry) || self.wiring != other.wiring
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
        // Face and standby defaults: the spectrum with everything, the clock,
        // dark after half an hour, 24-hour, address shown.
        assert_eq!(s.look, Look::default());
        assert!(s.idle_clock);
        assert_eq!(s.idle_timeout, Some(std::time::Duration::from_secs(1800)));
        assert_eq!(
            s.pause_standby,
            Some(std::time::Duration::ZERO),
            "a pause is a stop at once"
        );
        assert_eq!(
            (s.clock_format.as_str(), s.date_format.as_str()),
            ("%H:%M", "%a %d %b")
        );
        let text = store(&[
            ("connection", "\"spi\""),
            ("panel", "\"ssd1351_128x128\""),
            ("gpio_dc", "25"),
            ("face", "\"vu\""),
            ("track_text", "\"full\""),
            ("time_display", "\"remaining\""),
            ("show_format", "false"),
            ("peaks", "false"),
            ("meter_gain", "-6"),
            ("idle_screen", "\"blank\""),
            ("idle_timeout", "0"),
            ("pause_timeout", "45"),
            ("clock_format", "\"%I:%M %p\""),
            ("date_format", "\"%Y-%m-%d\""),
            ("show_ip", "false"),
            ("theme", "\"custom\""),
            ("colour_title", "\"#ff8000\""),
            ("colour_bar_low", "\"nonsense\""),
            ("meter_wide", "true"),
        ]);
        let s = Settings::parse(&text).expect("parses");
        assert_eq!(
            s.look,
            Look {
                face: Face::Vu,
                text: TrackText::Full,
                time: TimeDisplay::Remaining,
                show_format: false,
                peaks: false,
                gain_db: -6,
                show_ip: false,
                theme: Theme {
                    title: Colour::rgb(255, 128, 0),
                    ..Theme::default()
                },
                wide_meter: true,
            }
        );
        assert!(!s.idle_clock);
        assert_eq!(s.idle_timeout, None);
        assert_eq!(s.pause_standby, Some(std::time::Duration::from_secs(45)));
        assert_eq!(
            (s.clock_format.as_str(), s.date_format.as_str()),
            ("%I:%M %p", "%Y-%m-%d")
        );
        // A format that is not offered falls back on the first.
        let text = store(&[
            ("connection", "\"spi\""),
            ("panel", "\"ssd1351_128x128\""),
            ("gpio_dc", "25"),
            ("clock_format", "\"12h\""),
            ("face", "\"disco\""),
            ("meter_gain", "99"),
        ]);
        let s = Settings::parse(&text).expect("parses");
        assert_eq!(s.clock_format, "%H:%M");
        assert_eq!(s.look.face, Face::Spectrum);
        assert_eq!(s.look.gain_db, 12, "held to the range");
        let text = store(&[
            ("connection", "\"spi\""),
            ("panel", "\"ssd1351_128x128\""),
            ("gpio_dc", "25"),
            ("theme", "\"amber\""),
            ("pause_standby", "false"),
            ("pause_timeout", "99999"),
        ]);
        let s = Settings::parse(&text).expect("parses");
        assert_eq!(s.look.theme, Theme::preset("amber"));
        assert_eq!(s.pause_standby, None, "switched off: the pause text stays");
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
