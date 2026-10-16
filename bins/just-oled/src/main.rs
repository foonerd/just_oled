//! The display daemon. This build answers `--version`, `--help`, `--list`
//! (the catalogue, as text or JSON), `--probe` (the machine's buses and
//! the settings, side by side) and `--test` (the test picture on the
//! configured panel); the ring reader, the control socket and the pages
//! arrive stage by stage.

mod pattern;
mod probe;
mod settings;

use std::process::ExitCode;
use std::time::Duration;

use justoled::catalog::{self, Entry};
use justoled::oled::Oled;

use settings::Settings;

const USAGE: &str = "\
just-oled: the Just OLED display daemon

  just-oled --list [--json]   every panel the daemon knows
  just-oled --probe           the buses and chips this machine has, and the settings
  just-oled --test [--hold S] the test picture on the configured panel, held S seconds
                              (default 10; 0 leaves it up)
  just-oled --config PATH     read the settings from PATH rather than Volumio's store
  just-oled --version
  just-oled --help
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let has = |flag: &str| args.iter().any(|a| a == flag);
    let value = |flag: &str| {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    if has("--help") || has("-h") {
        print!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    if has("--version") {
        println!("just-oled {}", justoled::VERSION);
        return ExitCode::SUCCESS;
    }
    if has("--list") {
        if has("--json") {
            print!("{}", list_json());
        } else {
            print!("{}", list_text());
        }
        return ExitCode::SUCCESS;
    }
    let config = value("--config").unwrap_or_else(|| settings::DEFAULT_PATH.to_string());
    if has("--probe") {
        let loaded = load_settings(&config);
        print!("{}", probe::report(loaded.as_ref().map_err(String::as_str)));
        return ExitCode::SUCCESS;
    }
    if has("--test") {
        let hold = value("--hold")
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(10);
        return match show_test(&config, hold) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("just-oled: {e}");
                ExitCode::FAILURE
            }
        };
    }
    if args.is_empty() {
        eprintln!("just-oled: the daemon proper is not in this build; see --help");
        return ExitCode::from(2);
    }
    eprintln!("just-oled: unknown arguments: {}", args.join(" "));
    eprint!("{USAGE}");
    ExitCode::from(2)
}

fn load_settings(path: &str) -> Result<Settings, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    Settings::parse(&text).map_err(|e| format!("{path}: {e}"))
}

/// Open the configured panel, draw the test picture, hold it, then blank
/// and switch the display off; `hold` 0 leaves the picture up.
fn show_test(config: &str, hold: u64) -> Result<(), String> {
    let s = load_settings(config)?;
    let mut oled =
        Oled::open(s.entry, &s.wiring).map_err(|e| format!("open {}: {e}", s.entry.id))?;
    oled.init()
        .map_err(|e| format!("init {}: {e}", s.entry.id))?;
    let mut canvas = oled.canvas().with_rotate(s.rotate);
    pattern::draw(&mut canvas);
    let started = std::time::Instant::now();
    oled.show(&canvas).map_err(|e| format!("show: {e}"))?;
    println!(
        "just-oled: test picture on {} ({}x{}) via {:?}, frame sent in {:.1} ms",
        s.entry.id,
        s.entry.width,
        s.entry.height,
        s.wiring,
        started.elapsed().as_secs_f64() * 1000.0
    );
    if hold > 0 {
        std::thread::sleep(Duration::from_secs(hold));
        canvas.clear();
        oled.show(&canvas).map_err(|e| format!("clear: {e}"))?;
        oled.off().map_err(|e| format!("off: {e}"))?;
    }
    Ok(())
}

/// The catalogue as a table, one line per panel.
fn list_text() -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "{:<18} {:<9} {:<9} {:<10} {:<9} {:<10} {}\n",
        "id", "chip", "size", "format", "buses", "i2c", "status"
    ));
    for e in catalog::ENTRIES {
        out.push_str(&format!(
            "{:<18} {:<9} {:<9} {:<10} {:<9} {:<10} {}\n",
            e.id,
            e.controller.name(),
            format!("{}x{}", e.width, e.height),
            e.format().name(),
            buses(e),
            addresses(e),
            e.verified.name()
        ));
    }
    out
}

fn buses(e: &Entry) -> String {
    e.buses
        .iter()
        .map(|b| b.name())
        .collect::<Vec<_>>()
        .join(",")
}

fn addresses(e: &Entry) -> String {
    if e.i2c_addresses.is_empty() {
        "-".to_string()
    } else {
        e.i2c_addresses
            .iter()
            .map(|a| format!("{a:#04x}"))
            .collect::<Vec<_>>()
            .join(",")
    }
}

/// The catalogue as JSON, for the wiki's generated pages and drawings.
fn list_json() -> String {
    let mut out = String::from("[\n");
    for (i, e) in catalog::ENTRIES.iter().enumerate() {
        let buses: Vec<String> = e.buses.iter().map(|b| json_str(b.name())).collect();
        let addrs: Vec<String> = e.i2c_addresses.iter().map(|a| a.to_string()).collect();
        out.push_str(&format!(
            "  {{\"id\":{},\"controller\":{},\"width\":{},\"height\":{},\"format\":{},\"buses\":[{}],\"i2c_addresses\":[{}],\"verified\":{},\"note\":{}}}{}\n",
            json_str(e.id),
            json_str(e.controller.name()),
            e.width,
            e.height,
            json_str(e.format().name()),
            buses.join(","),
            addrs.join(","),
            json_str(e.verified.name()),
            json_str(e.note),
            if i + 1 < catalog::ENTRIES.len() { "," } else { "" }
        ));
    }
    out.push_str("]\n");
    out
}

/// A JSON string literal: quotes, backslashes and control characters escaped.
fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use justoled::catalog::Bus;

    #[test]
    fn the_text_list_names_every_entry_once() {
        let text = list_text();
        for e in catalog::ENTRIES {
            assert_eq!(text.matches(&format!("\n{} ", e.id)).count(), 1, "{}", e.id);
        }
    }

    #[test]
    fn the_json_list_is_well_formed_enough_to_count() {
        let json = list_json();
        assert!(json.starts_with("[\n") && json.ends_with("]\n"));
        assert_eq!(json.matches("\"id\":").count(), catalog::ENTRIES.len());
        assert!(json.contains("\"note\":\"0.96\\\" and 1.3\\\" modules"));
    }

    #[test]
    fn json_strings_escape_what_json_needs() {
        assert_eq!(json_str("a\"b\\c\nd"), "\"a\\\"b\\\\c\\nd\"");
        assert_eq!(json_str("\u{1}"), "\"\\u0001\"");
    }

    #[test]
    fn colour_panels_are_spi_only() {
        let spi_only = |id: &str| catalog::find(id).map(|e| e.buses == [Bus::Spi]);
        assert_eq!(spi_only("ssd1351_128x128"), Some(true));
        assert_eq!(spi_only("ssd1306_128x64"), Some(false));
    }
}
