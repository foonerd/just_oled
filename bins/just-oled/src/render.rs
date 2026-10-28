//! `--render DIR`: every page the daemon draws, on each panel class, as
//! PPM files (binary P6, one pixel per panel pixel). For the pictures in
//! the documentation and for a look at a layout without a panel; the
//! journal's tool turns them into PNG at a readable size.

use std::io::Write;
use std::path::Path;
use std::time::{Duration, Instant};

use justoled::canvas::Canvas;
use justoled::catalog::Format;
use justoled::ring::Frame;

use crate::control::{Status, Track};
use crate::meter::Bars;
use crate::net::{Connection, Net};
use crate::pages::{self, Face, Look, Marquee, Theme, TimeDisplay, TrackText, View, THEMES};
use crate::pattern;

/// The panel classes the pages are laid out for.
const CLASSES: &[(&str, Format, u16, u16)] = &[
    ("128x32-mono", Format::Paged1, 128, 32),
    ("128x64-mono", Format::Paged1, 128, 64),
    ("256x64-grey", Format::Grey4, 256, 64),
    ("128x128-rgb", Format::Rgb565, 128, 128),
    ("96x64-rgb", Format::Rgb565, 96, 64),
    ("320x240-rgb", Format::Rgb565, 320, 240),
];

/// The faces at the default look, then the spectrum face as the other
/// choices shape it; the name each file carries.
fn looks() -> Vec<(&'static str, Look)> {
    let with = |face: Face, text: TrackText, time: TimeDisplay, show_format: bool| Look {
        face,
        text,
        time,
        show_format,
        ..Look::default()
    };
    let dflt = Look::default();
    vec![
        ("spectrum", with(Face::Spectrum, dflt.text, dflt.time, true)),
        (
            "spectrum-stereo",
            with(Face::SpectrumStereo, dflt.text, dflt.time, true),
        ),
        (
            "spectrum-vu",
            with(Face::SpectrumVu, dflt.text, dflt.time, true),
        ),
        ("vu", with(Face::Vu, dflt.text, dflt.time, true)),
        // The text face shows the time left, so the row is seen both ways.
        (
            "text",
            with(Face::Text, dflt.text, TimeDisplay::Remaining, true),
        ),
        ("meter", with(Face::MeterOnly, dflt.text, dflt.time, true)),
        (
            "spectrum-full-text",
            with(Face::Spectrum, TrackText::Full, dflt.time, true),
        ),
        (
            "spectrum-title-only",
            with(Face::Spectrum, TrackText::Title, dflt.time, true),
        ),
        (
            "spectrum-no-format",
            with(Face::Spectrum, dflt.text, dflt.time, false),
        ),
        (
            "spectrum-bare",
            with(Face::Spectrum, TrackText::Title, TimeDisplay::Off, false),
        ),
        (
            "spectrum-wide",
            Look {
                wide_meter: true,
                ..Look::default()
            },
        ),
    ]
}

/// Scenes written per class: the lifecycle texts, the looks, the standby pages.
const SCENES_PER_CLASS: usize = 6 + 11 + 3;

fn bars_for(width: u16, height: u16, look: &Look) -> Bars {
    let mut bars = Bars::new(pages::bar_count(width, height, look));
    // A spectrum shape that reads as music: a bass hump and a presence
    // lift, the right channel a little quieter so the stereo faces differ.
    let shape = |gain: f32| -> Vec<f32> {
        (0..64)
            .map(|i| {
                let x = i as f32 / 63.0;
                let hump = (-((x - 0.15) * 6.0).powi(2)).exp();
                let lift = 0.5 * (-((x - 0.7) * 4.0).powi(2)).exp();
                (0.02 + 0.9 * hump + lift).min(1.0) * gain
            })
            .collect()
    };
    let frame = Frame {
        spectrum: [shape(0.8), shape(0.6)],
        peak: [0.7, 0.55],
        rms: [0.25, 0.18],
        ..Frame::default()
    };
    bars.feed(&frame, Duration::from_millis(40));
    // One frame later, so the peak marks sit just above the bars.
    bars.decay(Duration::from_millis(40));
    bars
}

fn write_ppm(path: &Path, canvas: &Canvas) -> std::io::Result<()> {
    let (w, h) = (canvas.width() as i32, canvas.height() as i32);
    let mut out = Vec::with_capacity((w * h * 3) as usize + 32);
    write!(out, "P6\n{w} {h}\n255\n")?;
    for y in 0..h {
        for x in 0..w {
            let c = canvas.get(x, y);
            out.extend_from_slice(&[c.r, c.g, c.b]);
        }
    }
    std::fs::write(path, out)
}

pub fn render(dir: &Path) -> std::io::Result<usize> {
    std::fs::create_dir_all(dir)?;
    let track = Track {
        status: Status::Play,
        title: "Heart of Gold".into(),
        artist: "Neil Young".into(),
        album: "Harvest".into(),
        samplerate: "44.1 kHz".into(),
        bitdepth: "16 bit".into(),
        bitrate: "1411 kbps".into(),
        volume: 62,
        seek: 83_000,
        duration: 187,
        ..Track::default()
    };
    let net = Net {
        connection: Connection::Wifi(3),
        ip: "192.168.1.23".into(),
    };
    let now = Instant::now();
    let mut rows: [Marquee; 4] = Default::default();
    rows[0].set(&track.album, now);
    rows[1].set(&track.artist, now);
    rows[2].set(&pages::artist_album(&track), now);
    rows[3].set(&track.title, now);
    let date = "Thu 16 Oct 2026";
    let mut written = 0;
    for &(class, format, w, h) in CLASSES {
        let mut scenes: Vec<(String, Canvas)> = Vec::with_capacity(SCENES_PER_CLASS + 4);
        let fresh = || Canvas::new(format, w, h);
        let theme = Theme::default();

        let mut c = fresh();
        pages::lines(&mut c, &["Volumio".into(), "booting...".into()], &theme);
        scenes.push(("boot".into(), c));

        let mut c = fresh();
        pages::lines(&mut c, &["Volumio".into(), "ready".into()], &theme);
        scenes.push(("ready".into(), c));

        for (name, look) in looks() {
            let bars = bars_for(w, h, &look);
            let view = View {
                track: &track,
                bars: &bars,
                blind: false,
                elapsed_s: 83,
                net: &net,
                look: &look,
                album: &rows[0],
                artist: &rows[1],
                line: &rows[2],
                title: &rows[3],
                now,
            };
            let mut c = fresh();
            pages::playing(&mut c, &view);
            scenes.push((format!("face-{name}"), c));
        }

        // The other themes, on the panels that have colour.
        if format == Format::Rgb565 {
            for name in THEMES
                .iter()
                .filter(|t| !matches!(**t, "classic" | "custom"))
            {
                let look = Look {
                    theme: Theme::preset(name),
                    ..Look::default()
                };
                let bars = bars_for(w, h, &look);
                let view = View {
                    track: &track,
                    bars: &bars,
                    blind: false,
                    elapsed_s: 83,
                    net: &net,
                    look: &look,
                    album: &rows[0],
                    artist: &rows[1],
                    line: &rows[2],
                    title: &rows[3],
                    now,
                };
                let mut c = fresh();
                pages::playing(&mut c, &view);
                scenes.push((format!("theme-{name}"), c));
            }
        }

        let look = Look::default();
        let silent = Bars::new(pages::bar_count(w, h, &look));
        let blind_view = View {
            track: &track,
            bars: &silent,
            blind: true,
            elapsed_s: 83,
            net: &net,
            look: &look,
            album: &rows[0],
            artist: &rows[1],
            line: &rows[2],
            title: &rows[3],
            now,
        };
        let mut c = fresh();
        pages::playing(&mut c, &blind_view);
        scenes.push(("playing-no-tap".into(), c));

        let mut c = fresh();
        pages::lines(
            &mut c,
            &["Paused".into(), "waiting on music".into()],
            &theme,
        );
        scenes.push(("paused".into(), c));

        let mut c = fresh();
        pages::standby(&mut c, "20:45", date, &net, &look);
        scenes.push(("standby".into(), c));

        let mut c = fresh();
        pages::standby(&mut c, "8:45 pm", "Thursday 16 October", &net, &look);
        scenes.push(("standby-12h".into(), c));

        let quiet = Look {
            show_ip: false,
            ..Look::default()
        };
        let mut c = fresh();
        pages::standby(&mut c, "20:45:30", "16.10.2026", &net, &quiet);
        scenes.push(("standby-no-ip".into(), c));

        let mut c = fresh();
        pages::lines(&mut c, &["Volumio".into(), "shutting down".into()], &theme);
        scenes.push(("farewell".into(), c));

        let mut c = fresh();
        pattern::draw(&mut c);
        scenes.push(("test".into(), c));

        for (name, canvas) in scenes {
            write_ppm(&dir.join(format!("{class}-{name}.ppm")), &canvas)?;
            written += 1;
        }
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_scene_renders_on_every_class() {
        let dir = std::env::temp_dir().join(format!("just-oled-render-{}", std::process::id()));
        let n = render(&dir).expect("render");
        let colour_classes = CLASSES.iter().filter(|c| c.1 == Format::Rgb565).count();
        assert_eq!(n, CLASSES.len() * SCENES_PER_CLASS + colour_classes * 4);
        let one = std::fs::read(dir.join("128x128-rgb-face-spectrum.ppm")).unwrap();
        assert!(one.starts_with(b"P6\n128 128\n255\n"));
        assert_eq!(one.len(), "P6\n128 128\n255\n".len() + 128 * 128 * 3);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
