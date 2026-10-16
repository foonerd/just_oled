//! `--render DIR`: every page the daemon draws, on each panel class, as
//! PPM files (binary P6, one pixel per panel pixel). For the pictures in
//! the documentation and for a look at a layout without a panel; the
//! journal's tool turns them into PNG at a readable size.

use std::io::Write;
use std::path::Path;
use std::time::Duration;

use justoled::canvas::Canvas;
use justoled::catalog::Format;
use justoled::ring::Frame;

use crate::control::{Status, Track};
use crate::meter::Bars;
use crate::pages;
use crate::pattern;

/// The panel classes the pages are laid out for.
const CLASSES: &[(&str, Format, u16, u16)] = &[
    ("128x32-mono", Format::Paged1, 128, 32),
    ("128x64-mono", Format::Paged1, 128, 64),
    ("256x64-grey", Format::Grey4, 256, 64),
    ("128x128-rgb", Format::Rgb565, 128, 128),
    ("96x64-rgb", Format::Rgb565, 96, 64),
];

fn bars_for(width: u16) -> Bars {
    let mut bars = Bars::new(pages::bar_count(width));
    // A spectrum shape that reads as music: a bass hump and a presence lift.
    let bands: Vec<f32> = (0..64)
        .map(|i| {
            let x = i as f32 / 63.0;
            let hump = (-((x - 0.15) * 6.0).powi(2)).exp();
            let lift = 0.5 * (-((x - 0.7) * 4.0).powi(2)).exp();
            (0.02 + 0.9 * hump + lift).min(1.0) * 0.8
        })
        .collect();
    let frame = Frame {
        spectrum: [bands.clone(), bands],
        peak: [0.7, 0.65],
        rms: [0.25, 0.22],
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
        title: "Harvest Moon".into(),
        artist: "Neil Young".into(),
        album: "Harvest Moon".into(),
        samplerate: "44.1 kHz".into(),
        bitdepth: "16 bit".into(),
        ..Track::default()
    };
    let mut written = 0;
    for &(class, format, w, h) in CLASSES {
        let bars = bars_for(w);
        let mut scenes: Vec<(&str, Canvas)> = Vec::new();
        let fresh = || Canvas::new(format, w, h);

        let mut c = fresh();
        pages::lines(&mut c, &["Volumio".into(), "booting...".into()]);
        scenes.push(("boot", c));

        let mut c = fresh();
        pages::lines(&mut c, &["Volumio".into(), "ready".into()]);
        scenes.push(("ready", c));

        let mut c = fresh();
        pages::playing(&mut c, &track, &bars, false);
        scenes.push(("playing", c));

        let mut c = fresh();
        pages::playing(&mut c, &track, &Bars::new(pages::bar_count(w)), true);
        scenes.push(("playing-no-tap", c));

        let mut c = fresh();
        pages::lines(&mut c, &["Paused".into(), "waiting on music".into()]);
        scenes.push(("paused", c));

        let mut c = fresh();
        pages::standby(&mut c, 20, 45);
        scenes.push(("standby", c));

        let mut c = fresh();
        pages::lines(&mut c, &["Volumio".into(), "shutting down".into()]);
        scenes.push(("farewell", c));

        let mut c = fresh();
        pattern::draw(&mut c);
        scenes.push(("test", c));

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
        assert_eq!(n, CLASSES.len() * 8);
        let one = std::fs::read(dir.join("128x128-rgb-playing.ppm")).unwrap();
        assert!(one.starts_with(b"P6\n128 128\n255\n"));
        assert_eq!(one.len(), "P6\n128 128\n255\n".len() + 128 * 128 * 3);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
