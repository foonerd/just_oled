//! The control socket: the plugin tells the daemon the player's state,
//! asks for a message, a test picture, a reload or a farewell, and may ask
//! how it is. One JSON object per line over a Unix socket; the daemon
//! answers only a `ping`.
//!
//! From the plugin:
//! - `{"kind":"state","status":"play|pause|stop","title":..,"artist":..,"album":..,
//!   "service":..,"trackType":..,"samplerate":..,"bitdepth":..,"bitrate":..,"volume":N,
//!   "mute":B,"seek":ms,"duration":s}`
//! - `{"kind":"ready"}` once the plugin has started
//! - `{"kind":"message","lines":["row 1","row 2"],"hold_ms":2000}` or `"text":"row 1|row 2"`
//! - `{"kind":"test","hold_ms":4000}`: the test picture, then back
//! - `{"kind":"farewell","reason":"shutdown|reboot"}` before the system goes down
//! - `{"kind":"reload"}` after the settings changed
//! - `{"kind":"ping"}`, answered with `{"kind":"pong", ...}`

use std::collections::BTreeMap;
use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use justoled::json::Value;

/// The service's runtime directory, made by systemd for the daemon's user
/// and removed when the service stops.
pub const RUN_DIR: &str = "/run/just-oled";
/// Where the socket goes when the daemon runs outside systemd.
pub const FALLBACK_PATH: &str = "/tmp/just-oled.sock";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Status {
    Play,
    Pause,
    #[default]
    Stop,
}

impl Status {
    pub fn name(self) -> &'static str {
        match self {
            Status::Play => "play",
            Status::Pause => "pause",
            Status::Stop => "stop",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Farewell {
    Shutdown,
    Reboot,
}

/// What is playing, as the plugin reduces Volumio's state.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Track {
    pub status: Status,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub service: String,
    pub track_type: String,
    pub samplerate: String,
    pub bitdepth: String,
    /// As Volumio reports it, "320 kbps"; empty when it does not.
    pub bitrate: String,
    pub volume: i64,
    pub mute: bool,
    /// Milliseconds into the track.
    pub seek: i64,
    /// Seconds.
    pub duration: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    State(Box<Track>),
    Ready,
    Message { lines: Vec<String>, hold: Duration },
    Test { hold: Duration },
    Farewell(Farewell),
    Reload,
}

/// What the daemon says of itself to a `ping`.
#[derive(Clone, Debug, Default)]
pub struct Report {
    pub panel: Option<String>,
    pub panel_error: Option<String>,
    pub ring: bool,
    pub phase: String,
    pub status: String,
    /// Frames sent per second lately, and the last frame's transfer time.
    pub fps: f32,
    pub frame_ms: f32,
}

/// `row 1|row 2` into rows, trimmed.
pub fn lines(text: &str) -> Vec<String> {
    text.split('|').map(|s| s.trim().to_string()).collect()
}

/// One line from the plugin as a command, or `None` for a line that is
/// not one (a ping included).
pub fn parse(line: &str) -> Option<Command> {
    let v = Value::parse(line).ok()?;
    let kind = v.get("kind")?.as_str()?;
    let text = |key: &str| v.get(key).and_then(Value::as_str).unwrap_or("").to_string();
    let number = |key: &str| -> i64 {
        match v.get(key) {
            Some(Value::Number(n)) => *n as i64,
            Some(Value::String(s)) => s.trim().parse().unwrap_or(0),
            _ => 0,
        }
    };
    let hold = |default: u64| {
        let ms = match v.get("hold_ms") {
            Some(Value::Number(n)) if *n >= 0.0 => *n as u64,
            _ => default,
        };
        Duration::from_millis(ms.clamp(100, 600_000))
    };
    match kind {
        "state" => Some(Command::State(Box::new(Track {
            status: match text("status").as_str() {
                "play" => Status::Play,
                "pause" => Status::Pause,
                _ => Status::Stop,
            },
            title: text("title"),
            artist: text("artist"),
            album: text("album"),
            service: text("service"),
            track_type: text("trackType"),
            samplerate: text("samplerate"),
            bitdepth: text("bitdepth"),
            bitrate: text("bitrate"),
            volume: number("volume"),
            mute: v.get("mute").and_then(Value::as_bool).unwrap_or(false),
            seek: number("seek"),
            duration: number("duration"),
        }))),
        "ready" => Some(Command::Ready),
        "message" => {
            let rows = match v.get("lines").and_then(Value::as_array) {
                Some(rows) => rows
                    .iter()
                    .map(|r| r.as_str().unwrap_or("").to_string())
                    .collect(),
                None => lines(&text("text")),
            };
            Some(Command::Message {
                lines: rows,
                hold: hold(2000),
            })
        }
        "test" => Some(Command::Test { hold: hold(4000) }),
        "farewell" => Some(Command::Farewell(match text("reason").as_str() {
            "reboot" | "restart" => Farewell::Reboot,
            _ => Farewell::Shutdown,
        })),
        "reload" => Some(Command::Reload),
        _ => None,
    }
}

fn is_ping(line: &str) -> bool {
    Value::parse(line)
        .ok()
        .and_then(|v| v.get("kind").and_then(Value::as_str).map(|k| k == "ping"))
        .unwrap_or(false)
}

fn pong(r: &Report) -> String {
    let opt = |s: &Option<String>| s.clone().map_or(Value::Null, Value::String);
    let mut m = BTreeMap::new();
    m.insert("kind".into(), Value::String("pong".into()));
    m.insert("version".into(), Value::String(justoled::VERSION.into()));
    // The process: a client that sees another one knows the daemon started anew.
    m.insert("pid".into(), Value::Number(f64::from(std::process::id())));
    m.insert("panel".into(), opt(&r.panel));
    m.insert("panelError".into(), opt(&r.panel_error));
    m.insert("ring".into(), Value::Bool(r.ring));
    m.insert("phase".into(), Value::String(r.phase.clone()));
    m.insert("status".into(), Value::String(r.status.clone()));
    m.insert(
        "fps".into(),
        Value::Number((r.fps * 10.0).round() as f64 / 10.0),
    );
    m.insert(
        "frameMs".into(),
        Value::Number((r.frame_ms * 10.0).round() as f64 / 10.0),
    );
    Value::Object(m).to_json()
}

/// Listen on `path` on a thread of its own, handing every command to `tx`
/// and answering pings from `report`. A socket file left by an earlier
/// run is replaced.
pub fn serve(path: &Path, tx: Sender<Command>, report: Arc<Mutex<Report>>) -> io::Result<()> {
    let _ = std::fs::remove_file(path);
    let listener = UnixListener::bind(path)?;
    let path = path.to_path_buf();
    std::thread::Builder::new()
        .name("control".into())
        .spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let tx = tx.clone();
                let report = report.clone();
                let _ = std::thread::Builder::new()
                    .name("control-conn".into())
                    .spawn(move || connection(stream, tx, report));
            }
            let _ = std::fs::remove_file(&path);
        })?;
    Ok(())
}

fn connection(stream: UnixStream, tx: Sender<Command>, report: Arc<Mutex<Report>>) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
    let Ok(mut writer) = stream.try_clone() else {
        return;
    };
    let reader = BufReader::new(stream);
    for line in reader.lines() {
        let Ok(line) = line else { break };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if is_ping(line) {
            let r = report.lock().map(|r| r.clone()).unwrap_or_default();
            if writeln!(writer, "{}", pong(&r)).is_err() {
                break;
            }
            continue;
        }
        match parse(line) {
            Some(cmd) => {
                if tx.send(cmd).is_err() {
                    break;
                }
            }
            None => eprintln!("just-oled: control: not a command: {line}"),
        }
    }
}

/// Send one line to the daemon at `path`; with `reply`, wait for one line back.
pub fn send(path: &Path, message: &str, reply: bool) -> io::Result<Option<String>> {
    let mut stream = UnixStream::connect(path)?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    writeln!(stream, "{message}")?;
    stream.flush()?;
    if !reply {
        return Ok(None);
    }
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line)?;
    Ok(Some(line.trim().to_string()))
}

/// The socket's path: under the runtime directory while the service has
/// one, else under /tmp. Daemon and plugin agree because the directory
/// exists exactly while the service runs.
pub fn default_path() -> PathBuf {
    let dir = Path::new(RUN_DIR);
    if dir.is_dir() {
        dir.join("just-oled.sock")
    } else {
        PathBuf::from(FALLBACK_PATH)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn lines_parse_into_commands() {
        let state = parse(
            r#"{"kind":"state","status":"play","title":"Song","artist":"Band","album":"LP","service":"mpd","trackType":"flac","samplerate":"44.1 kHz","bitdepth":"16 bit","volume":42,"mute":false,"seek":1234,"duration":300}"#,
        );
        let Some(Command::State(t)) = state else {
            panic!("{state:?}");
        };
        assert_eq!(t.status, Status::Play);
        assert_eq!((t.title.as_str(), t.artist.as_str()), ("Song", "Band"));
        assert_eq!((t.volume, t.seek, t.duration), (42, 1234, 300));
        assert_eq!(parse(r#"{"kind":"ready"}"#), Some(Command::Ready));
        assert_eq!(
            parse(r#"{"kind":"message","text":"a | b","hold_ms":50}"#),
            Some(Command::Message {
                lines: vec!["a".into(), "b".into()],
                hold: Duration::from_millis(100)
            })
        );
        assert_eq!(
            parse(r#"{"kind":"message","lines":["x","y","z"]}"#),
            Some(Command::Message {
                lines: vec!["x".into(), "y".into(), "z".into()],
                hold: Duration::from_millis(2000)
            })
        );
        assert_eq!(
            parse(r#"{"kind":"test"}"#),
            Some(Command::Test {
                hold: Duration::from_millis(4000)
            })
        );
        assert_eq!(
            parse(r#"{"kind":"farewell","reason":"reboot"}"#),
            Some(Command::Farewell(Farewell::Reboot))
        );
        assert_eq!(
            parse(r#"{"kind":"farewell"}"#),
            Some(Command::Farewell(Farewell::Shutdown))
        );
        assert_eq!(parse(r#"{"kind":"reload"}"#), Some(Command::Reload));
        assert_eq!(parse(r#"{"kind":"ping"}"#), None);
        assert_eq!(parse("not json"), None);
        assert_eq!(
            parse(r#"{"kind":"state","status":"pause"}"#)
                .map(|c| matches!(c, Command::State(t) if t.status == Status::Pause)),
            Some(true)
        );
    }

    #[test]
    fn the_socket_takes_commands_and_answers_pings() {
        let path = std::env::temp_dir().join(format!("just-oled-test-{}.sock", std::process::id()));
        let (tx, rx) = mpsc::channel();
        let report = Arc::new(Mutex::new(Report {
            panel: Some("ssd1351_128x128".into()),
            phase: "player".into(),
            status: "play".into(),
            ring: true,
            ..Default::default()
        }));
        serve(&path, tx, report).expect("serve");
        assert_eq!(send(&path, r#"{"kind":"ready"}"#, false).unwrap(), None);
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(2)).unwrap(),
            Command::Ready
        );
        let answer = send(&path, r#"{"kind":"ping"}"#, true).unwrap().unwrap();
        let v = Value::parse(&answer).expect("json");
        assert_eq!(v.get("kind").and_then(Value::as_str), Some("pong"));
        assert_eq!(
            v.get("panel").and_then(Value::as_str),
            Some("ssd1351_128x128")
        );
        assert_eq!(v.get("panelError"), Some(&Value::Null));
        assert_eq!(v.get("ring").and_then(Value::as_bool), Some(true));
        assert_eq!(
            v.get("version").and_then(Value::as_str),
            Some(justoled::VERSION)
        );
        assert_eq!(
            v.get("pid").and_then(Value::as_f64),
            Some(f64::from(std::process::id()))
        );
        let _ = std::fs::remove_file(&path);
    }
}
