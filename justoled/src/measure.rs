//! What the tap measures from a stream once its shape is known: linear
//! audio through the analyser, one-bit audio by density, every hop
//! published into a ring of the stream's own. Fed with interleaved samples
//! on the 16-bit scale, one per channel per frame, or for DSD one per byte,
//! as the relay carries them. From glass's tap, without the demand file:
//! the bank's shape is fixed by the tap's configuration.

use std::path::{Path, PathBuf};

use crate::analysis::Analyser;
use crate::bank::Demand;
use crate::ring::{Frame, Writer, MAX_CHANNELS};
use crate::sample::Layout;
use crate::{dop, dsd};

/// The shape of a stream, from the player's hw_params.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shape {
    pub rate: u32,
    pub channels: u32,
    pub layout: Layout,
}

/// How the measurements are made and where they go.
#[derive(Clone, Debug)]
pub struct Settings {
    /// The tag in the ring's file name.
    pub ring: String,
    /// Frames between measurements; zero means the bank's own hop.
    pub hop: usize,
    /// Hops the ring keeps.
    pub slots: usize,
    /// The bank's shape.
    pub demand: Demand,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            ring: "justoled".to_string(),
            hop: 0,
            slots: 16,
            demand: Demand::default(),
        }
    }
}

pub struct Measure {
    settings: Settings,
    dir: PathBuf,
    shape: Option<Shape>,
    analyser: Option<Analyser>,
    writer: Option<Writer>,
    /// The frame published for one-bit audio: a level, no spectrum.
    quiet: Frame,
    /// Bytes per density group for one-bit audio, frames for DoP.
    group: usize,
    frames: u64,
    chans: [Vec<i16>; MAX_CHANNELS],
}

impl Measure {
    /// Rings go under `dir`.
    pub fn new(dir: impl Into<PathBuf>, settings: Settings) -> Self {
        Self {
            settings,
            dir: dir.into(),
            shape: None,
            analyser: None,
            writer: None,
            quiet: Frame {
                one_bit: true,
                ..Frame::default()
            },
            group: 0,
            frames: 0,
            chans: [Vec::new(), Vec::new()],
        }
    }

    pub fn shape(&self) -> Option<Shape> {
        self.shape
    }

    /// The demand the bank is measured to.
    pub fn demand(&self) -> Demand {
        self.settings.demand
    }

    /// The ring's file while a stream has one.
    pub fn ring_path(&self) -> Option<&Path> {
        self.writer.as_ref().map(|w| w.path())
    }

    /// A stream of this shape starts, or none: the old ring goes, a new one
    /// is made. Returns what went wrong making the ring, if anything; the
    /// measuring goes on without it.
    pub fn set_shape(&mut self, shape: Option<Shape>) -> Result<(), String> {
        self.shape = shape;
        self.analyser = None;
        self.writer = None;
        self.frames = 0;
        let Some(s) = shape else {
            return Ok(());
        };
        let analyser = Analyser::new(s.rate, s.channels, self.settings.demand, self.settings.hop);
        self.quiet = Frame {
            spectrum: [vec![0.0; analyser.bins()], vec![0.0; analyser.bins()]],
            hold: [vec![0.0; analyser.bins()], vec![0.0; analyser.bins()]],
            scale: analyser.demand().scale,
            window: analyser.fft_size() as u32,
            one_bit: true,
            ..Frame::default()
        };
        self.group = if s.layout.is_dsd() {
            dsd::group_bytes(s.rate as u64 * s.layout.bytes() as u64 * 8)
        } else {
            dop::group_frames(s.rate)
        };
        let made = Writer::create(
            &self.dir,
            &self.settings.ring,
            s.rate,
            s.channels,
            analyser.demand(),
            analyser.hop() as u32,
            self.settings.slots,
        );
        self.analyser = Some(analyser);
        match made {
            Ok(w) => {
                self.writer = Some(w);
                Ok(())
            }
            Err(e) => Err(format!("no ring: {e}")),
        }
    }

    /// Forget the stream so far; the next hop starts from silence.
    pub fn reset(&mut self) {
        if let Some(a) = self.analyser.as_mut() {
            a.reset();
        }
    }

    /// Frames between measurements for the stream on hand.
    pub fn hop(&self) -> usize {
        self.analyser
            .as_ref()
            .map(|a| a.hop())
            .unwrap_or(crate::bank::HOP)
    }

    /// Measure `samples`, interleaved as the stream's shape says, and hand
    /// `sink` every hop they complete, with how many of these samples'
    /// frames had gone in when the hop completed, so the caller can place
    /// the hop in time. Frames are words for one-bit audio.
    pub fn take(&mut self, samples: &[i16], sink: &mut dyn FnMut(&Frame, usize)) {
        let (Some(shape), Some(analyser)) = (self.shape, self.analyser.as_mut()) else {
            return;
        };
        let channels = (shape.channels as usize).max(1);
        let n = samples.len() / channels;
        if n == 0 {
            return;
        }
        let used = channels.min(MAX_CHANNELS);
        for (ch, out) in self.chans.iter_mut().enumerate().take(used) {
            out.clear();
            out.extend(samples.iter().skip(ch).step_by(channels).take(n));
        }
        let by_density = shape.layout.is_dsd() || dop::is_stream(&self.chans[0]);
        if by_density {
            // One-bit audio has no spectrum to show; the level stands in,
            // one per hop's worth of words so it moves as the audio does.
            let per_frame = if shape.layout.is_dsd() {
                shape.layout.bytes().max(1)
            } else {
                1
            };
            let block = (analyser.hop() * per_frame).max(1);
            let group = self.group;
            let level = |s: &[i16]| dsd::level(s.iter().map(|v| *v as u8), group) as f32 / 32767.0;
            let mut start = 0;
            while start < n {
                let end = (start + block).min(n);
                let left = level(&self.chans[0][start..end]);
                let right = if used > 1 {
                    level(&self.chans[1][start..end])
                } else {
                    left
                };
                self.frames += ((end - start) / per_frame) as u64;
                self.quiet.frames = self.frames;
                self.quiet.peak = [left, right];
                self.quiet.rms = [left, right];
                sink(&self.quiet, end / per_frame);
                start = end;
            }
            return;
        }
        let slices: [&[i16]; MAX_CHANNELS] = [&self.chans[0], &self.chans[1]];
        analyser.feed(&slices[..used], n, sink);
        self.frames += n as u64;
    }

    /// Put a frame into the stream's ring, if it has one.
    pub fn publish(&mut self, frame: &Frame) {
        if let Some(w) = self.writer.as_mut() {
            w.publish(frame);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ring::Reader;

    fn dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("justoled-measure-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn settings() -> Settings {
        Settings {
            ring: "test".into(),
            hop: 128,
            slots: 8,
            demand: Demand::default(),
        }
    }

    #[test]
    fn linear_audio_is_analysed_into_the_stream_s_ring() {
        let d = dir("linear");
        let mut m = Measure::new(&d, settings());
        assert!(m.ring_path().is_none());
        let shape = Shape {
            rate: 48_000,
            channels: 2,
            layout: Layout::from_alsa(2).unwrap(),
        };
        m.set_shape(Some(shape)).unwrap();
        let path = m.ring_path().unwrap().to_path_buf();
        assert!(path.exists());
        let mut samples = Vec::new();
        for i in 0..1024 {
            let s = (0.5
                * (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / 48_000.0).sin()
                * 32767.0) as i16;
            samples.push(s);
            samples.push(0);
        }
        let mut got: Vec<(Frame, usize)> = Vec::new();
        m.take(&samples, &mut |f, n| got.push((f.clone(), n)));
        for (f, _) in &got {
            m.publish(f);
        }
        let reader = Reader::open(&path).unwrap();
        assert_eq!(
            got.iter().map(|g| g.1).collect::<Vec<_>>(),
            (1..=8).map(|k| k * 128).collect::<Vec<_>>()
        );
        let frame = reader.latest().expect("a frame after eight hops");
        assert!(
            (frame.peak[0] - 0.5).abs() < 0.02,
            "left peak {}",
            frame.peak[0]
        );
        assert_eq!(frame.peak[1], 0.0);
        assert_eq!(frame.frames, 1024);
        assert_eq!(reader.info().rate, 48_000);
        assert_eq!(reader.info().bins, 64);
        m.set_shape(Some(Shape {
            rate: 44_100,
            ..shape
        }))
        .unwrap();
        assert!(
            !path.exists(),
            "a new shape makes a new ring and the old file goes"
        );
        assert_ne!(m.ring_path().unwrap(), path);
        m.set_shape(None).unwrap();
        assert!(m.ring_path().is_none());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn one_bit_audio_is_measured_by_density_and_dop_is_told_apart() {
        let d = dir("dsd");
        let mut m = Measure::new(&d, settings());
        let dsd = Shape {
            rate: 88_200,
            channels: 2,
            layout: Layout::from_alsa(50).unwrap(),
        };
        m.set_shape(Some(dsd)).unwrap();
        let path = m.ring_path().unwrap().to_path_buf();
        let group = dsd::group_bytes(88_200 * 32);
        let mut samples = Vec::new();
        for _ in 0..group * 8 {
            samples.push(0x55i16);
            samples.push(0xFFi16);
        }
        let mut got: Vec<(Frame, usize)> = Vec::new();
        m.take(&samples, &mut |f, n| got.push((f.clone(), n)));
        for (f, _) in &got {
            m.publish(f);
        }
        let frame = Reader::open(&path)
            .unwrap()
            .latest()
            .expect("a density frame");
        assert_eq!(frame.peak[0], 0.0);
        assert_eq!(frame.peak[1], 1.0);
        assert!(frame.one_bit);
        assert!(
            frame.spectrum[0].iter().all(|v| *v == 0.0),
            "no spectrum for one-bit audio"
        );
        assert_eq!(
            frame.frames,
            (group * 8 / 4) as u64,
            "frames are words of four bytes"
        );
        let dop = Shape {
            rate: 176_400,
            channels: 2,
            layout: Layout::from_alsa(32).unwrap(),
        };
        m.set_shape(Some(dop)).unwrap();
        let path = m.ring_path().unwrap().to_path_buf();
        let mut samples = Vec::new();
        for i in 0..2048 {
            let marker = if i % 2 == 0 { 0x05 } else { 0xFA };
            samples.push(((marker << 8) | 0xFF) as i16);
            samples.push(((marker << 8) | 0x55) as i16);
        }
        let mut got: Vec<(Frame, usize)> = Vec::new();
        m.take(&samples, &mut |f, n| got.push((f.clone(), n)));
        for (f, _) in &got {
            m.publish(f);
        }
        let frame = Reader::open(&path).unwrap().latest().expect("a DoP frame");
        assert_eq!(frame.peak[0], 1.0, "all ones is beyond full scale");
        assert_eq!(frame.peak[1], 0.0, "half density is silence");
        let _ = std::fs::remove_dir_all(&d);
    }
}
