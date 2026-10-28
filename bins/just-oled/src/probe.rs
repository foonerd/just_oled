//! `--probe`: what this machine offers a panel, and what the settings ask
//! of it, side by side. Nothing is written to any bus but the I2C address
//! probes, which the kernel's own `i2cdetect` would do the same way.

use std::fmt::Write as _;

use justoled::catalog;
use justoled::oled::Wiring;
use justoled::sys::{gpio, i2c, spi};

use crate::settings::Settings;

/// The seven-bit addresses the catalogue's I2C panels answer on.
fn catalogue_addresses() -> Vec<u8> {
    let mut all: Vec<u8> = catalog::ENTRIES
        .iter()
        .flat_map(|e| e.i2c_addresses.iter().copied())
        .collect();
    all.sort_unstable();
    all.dedup();
    all
}

/// The bus clock the device tree gave an I2C adapter, in kHz, as sysfs
/// exposes the node's `clock-frequency` (a big-endian cell); `None` off a
/// device-tree machine or for a software bus.
fn i2c_clock_khz(bus: u8) -> Option<u32> {
    let bytes = std::fs::read(format!(
        "/sys/class/i2c-adapter/i2c-{bus}/of_node/clock-frequency"
    ))
    .ok()?;
    let cell: [u8; 4] = bytes.get(..4)?.try_into().ok()?;
    Some(u32::from_be_bytes(cell) / 1000)
}

pub fn report(settings: Result<&Settings, &str>) -> String {
    let mut out = String::new();

    let _ = writeln!(out, "I2C buses");
    let buses = i2c::buses();
    if buses.is_empty() {
        let _ = writeln!(out, "  none under /dev (is dtparam=i2c_arm=on set?)");
    }
    for bus in buses {
        let found: Vec<String> = catalogue_addresses()
            .into_iter()
            .filter(|&a| i2c::Device::present(bus, a))
            .map(|a| format!("{a:#04x}"))
            .collect();
        let clock = i2c_clock_khz(bus).map_or(String::new(), |k| format!(" ({k} kHz)"));
        let _ = writeln!(
            out,
            "  /dev/i2c-{bus}{clock}: {}",
            if found.is_empty() {
                "no panel at a catalogued address".to_string()
            } else {
                format!("answers at {}", found.join(", "))
            }
        );
    }

    let _ = writeln!(out, "SPI devices");
    let devices = spi::devices();
    if devices.is_empty() {
        let _ = writeln!(out, "  none under /dev (is dtparam=spi=on set?)");
    }
    for (bus, cs) in devices {
        let _ = writeln!(out, "  /dev/spidev{bus}.{cs}");
    }

    let _ = writeln!(out, "GPIO chips");
    let chips = gpio::Chip::all();
    if chips.is_empty() {
        let _ = writeln!(out, "  none under /dev");
    }
    let header = gpio::Chip::header();
    for chip in &chips {
        let _ = writeln!(
            out,
            "  {}: {} ({}), {} lines{}",
            chip.path,
            chip.label,
            chip.name,
            chip.lines,
            if header.as_ref() == Some(chip) {
                " - the header"
            } else {
                ""
            }
        );
    }

    let _ = writeln!(out, "Settings");
    match settings {
        Err(e) => {
            let _ = writeln!(out, "  {e}");
        }
        Ok(s) => {
            let _ = writeln!(
                out,
                "  panel {} ({} {}x{}, {}), {}",
                s.entry.id,
                s.entry.controller,
                s.entry.width,
                s.entry.height,
                s.entry.format().name(),
                s.entry.verified.name()
            );
            match &s.wiring {
                Wiring::Spi {
                    bus,
                    cs,
                    dc,
                    reset,
                    backlight,
                } => {
                    let _ = writeln!(
                        out,
                        "  SPI /dev/spidev{bus}.{cs}, DC GPIO {dc}, reset {}{} - device {}",
                        reset.map_or("not wired".to_string(), |r| format!("GPIO {r}")),
                        if s.entry.controller.is_tft() {
                            format!(
                                ", backlight {}",
                                backlight.map_or("not wired".to_string(), |b| format!("GPIO {b}"))
                            )
                        } else {
                            String::new()
                        },
                        if spi::device_exists(*bus, *cs) {
                            "present"
                        } else {
                            "MISSING"
                        }
                    );
                }
                Wiring::I2c { bus, addr, .. } => {
                    let _ = writeln!(
                        out,
                        "  I2C /dev/i2c-{bus} address {addr:#04x} - bus {}, panel {}",
                        if i2c::bus_exists(*bus) {
                            "present"
                        } else {
                            "MISSING"
                        },
                        if i2c::Device::present(*bus, *addr) {
                            "answers"
                        } else {
                            "SILENT"
                        }
                    );
                }
            }
            let _ = writeln!(out, "  orientation {:?}", s.rotate);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_report_has_its_sections_without_hardware() {
        let text = report(Err("no settings file"));
        for section in ["I2C buses", "SPI devices", "GPIO chips", "Settings"] {
            assert!(text.contains(section), "{section}");
        }
        assert!(text.contains("no settings file"));
        assert!(catalogue_addresses().contains(&0x3C));
    }
}
