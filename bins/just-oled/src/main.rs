//! The display daemon. Run without arguments it is the service: it opens
//! the configured panel, shows the lifecycle texts, follows the player
//! through the plugin's control socket and the audio tap's ring, and
//! draws the pages. The flags are the tools around it: `--list` (the
//! catalogue), `--probe` (the machine's buses beside the settings),
//! `--test` (the test picture), `--say` and `--ping` (talk to the
//! running daemon).

mod control;
mod daemon;
mod lifecycle;
mod meter;
mod pages;
mod pattern;
mod probe;
mod settings;

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::mpsc;
use std::time::Duration;

use justoled::catalog::{self, Entry};
use justoled::oled::Oled;

use settings::Settings;

const USAGE: &str = "\
just-oled: the Just OLED display daemon

  just-oled                   run the daemon (what the service does)
  just-oled --list [--json]   every panel the daemon knows
  just-oled --probe           the buses and chips this machine has, and the settings
  just-oled --test [--hold S] the test picture on the configured panel, held S seconds
                              (default 10; 0 leaves it up)
  just-oled --say \"row 1|row 2\" [--hold S]   a message on the running daemon's panel
  just-oled --ping            ask the running daemon how it is
  just-oled --config PATH     read the settings from PATH rather than Volumio's store
  just-oled --socket PATH     the control socket (default under /run/just-oled, else /tmp)
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
    let socket = value("--socket")
        .map(PathBuf::from)
        .unwrap_or_else(control::default_path);
    if has("--probe") {
        let loaded = load_settings(&config);
        print!("{}", probe::report(loaded.as_ref().map_err(String::as_str)));
        return ExitCode::SUCCESS;
    }
    let hold = value("--hold").and_then(|s| s.parse::<u64>().ok());
    if has("--test") {
        // A running daemon owns the panel: it draws the picture; else this process does.
        let message = format!(
            r#"{{"kind":"test","hold_ms":{}}}"#,
            hold.map_or(10_000, |s| if s == 0 { 600_000 } else { s * 1000 })
        );
        if control::send(&socket, &message, false).is_ok() {
            println!("just-oled: test picture asked of the running daemon");
            return ExitCode::SUCCESS;
        }
        return match show_test(&config, hold.unwrap_or(10)) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("just-oled: {e}");
                ExitCode::FAILURE
            }
        };
    }
    if let Some(text) = value("--say") {
        let mut m = std::collections::BTreeMap::new();
        m.insert(
            "kind".to_string(),
            justoled::json::Value::String("message".into()),
        );
        m.insert("text".to_string(), justoled::json::Value::String(text));
        m.insert(
            "hold_ms".to_string(),
            justoled::json::Value::Number((hold.unwrap_or(3) * 1000) as f64),
        );
        return client(&socket, &justoled::json::Value::Object(m).to_json(), false);
    }
    if has("--ping") {
        return client(&socket, r#"{"kind":"ping"}"#, true);
    }
    // With only the paths given, or nothing, it is the service.
    let mut rest = args.clone();
    for flag in ["--config", "--socket"] {
        if let Some(i) = rest.iter().position(|a| a == flag) {
            rest.drain(i..(i + 2).min(rest.len()));
        }
    }
    if rest.is_empty() {
        return run(&config, &socket);
    }
    eprintln!("just-oled: unknown arguments: {}", args.join(" "));
    eprint!("{USAGE}");
    ExitCode::from(2)
}

/// The service: the socket up, the loop until told to leave. Settings
/// that are not there yet (the plugin not turned on) are waited for.
fn run(config: &str, socket: &std::path::Path) -> ExitCode {
    lifecycle::install();
    pages::tzset_now();
    daemon::log(&format!(
        "{} starting: settings {config}, socket {}, rings under {}",
        justoled::VERSION,
        socket.display(),
        daemon::ring_dir().display()
    ));
    let mut d = daemon::Daemon::new(None, PathBuf::from(config), daemon::ring_dir());
    d.reload();
    let (tx, rx) = mpsc::channel();
    if let Err(e) = control::serve(socket, tx, d.report.clone()) {
        eprintln!("just-oled: control socket {}: {e}", socket.display());
        return ExitCode::FAILURE;
    }
    d.run(rx);
    let _ = std::fs::remove_file(socket);
    ExitCode::SUCCESS
}

/// One line to the running daemon; the answer printed when one is due.
fn client(socket: &std::path::Path, message: &str, reply: bool) -> ExitCode {
    match control::send(socket, message, reply) {
        Ok(Some(answer)) => {
            println!("{answer}");
            ExitCode::SUCCESS
        }
        Ok(None) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("just-oled: {}: {e}", socket.display());
            ExitCode::FAILURE
        }
    }
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
