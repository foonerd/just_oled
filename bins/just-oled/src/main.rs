//! The display daemon. This build answers `--version`, `--help` and
//! `--list` (the catalogue, as text or JSON); the ring reader, the control
//! socket, the pages and the drivers arrive stage by stage.

use std::process::ExitCode;

use justoled::catalog::{self, Entry};

const USAGE: &str = "\
just-oled: the Just OLED display daemon

  just-oled --list [--json]   every panel the daemon knows
  just-oled --version
  just-oled --help
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let has = |flag: &str| args.iter().any(|a| a == flag);
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
    if args.is_empty() {
        eprintln!("just-oled: the daemon proper is not in this build; see --help");
        return ExitCode::from(2);
    }
    eprintln!("just-oled: unknown arguments: {}", args.join(" "));
    eprint!("{USAGE}");
    ExitCode::from(2)
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
