//! The bars: the tap's bank, grouped into as many bars as the panel
//! shows, in decibels over a fixed range, with instant attack, a timed
//! release and a peak mark that holds and falls. The two channels are
//! folded into one set of bars, the louder side winning.

use std::time::Duration;

use justoled::ring::Frame;

/// The quietest bar: a band this far below full scale draws nothing.
pub const FLOOR_DB: f32 = -60.0;
/// How fast a bar falls once the music does, in decibels per second.
pub const RELEASE_DB_PER_S: f32 = 90.0;
/// How long the peak mark stays before it falls.
pub const PEAK_HOLD: Duration = Duration::from_millis(800);
/// How fast the peak mark falls after its hold, in decibels per second.
pub const PEAK_FALL_DB_PER_S: f32 = 40.0;

#[derive(Clone, Debug)]
pub struct Bars {
    /// Bar heights 0..=1.
    level: Vec<f32>,
    /// Peak marks 0..=1.
    peak: Vec<f32>,
    /// Time since each peak was set.
    peak_age: Vec<Duration>,
    /// Peak and RMS of the louder channel, 0..=1.
    peak_level: f32,
    rms_level: f32,
}

/// Linear amplitude to a 0..=1 height over `FLOOR_DB`.
pub fn height(amplitude: f32) -> f32 {
    if amplitude <= 0.0 {
        return 0.0;
    }
    let db = 20.0 * amplitude.log10();
    ((db - FLOOR_DB) / -FLOOR_DB).clamp(0.0, 1.0)
}

impl Bars {
    pub fn new(count: usize) -> Bars {
        Bars {
            level: vec![0.0; count],
            peak: vec![0.0; count],
            peak_age: vec![Duration::ZERO; count],
            peak_level: 0.0,
            rms_level: 0.0,
        }
    }

    pub fn count(&self) -> usize {
        self.level.len()
    }

    pub fn levels(&self) -> &[f32] {
        &self.level
    }

    pub fn peaks(&self) -> &[f32] {
        &self.peak
    }

    /// The louder channel's peak, 0..=1 of full scale.
    pub fn peak_level(&self) -> f32 {
        self.peak_level
    }

    pub fn rms_level(&self) -> f32 {
        self.rms_level
    }

    /// A hop's bank into the bars: each bar takes the loudest band of its
    /// group, rising at once and never below where the release would put it.
    pub fn feed(&mut self, frame: &Frame, dt: Duration) {
        self.decay(dt);
        let bands = frame.spectrum[0].len().max(frame.spectrum[1].len());
        if bands == 0 {
            // One-bit audio: no bank; the level alone moves.
            self.peak_level = self.peak_level.max(frame.peak[0].max(frame.peak[1]));
            self.rms_level = self.rms_level.max(frame.rms[0].max(frame.rms[1]));
            return;
        }
        let n = self.level.len();
        for (i, level) in self.level.iter_mut().enumerate() {
            let (from, to) = group(i, n, bands);
            let mut loudest = 0.0f32;
            for ch in &frame.spectrum {
                for &v in ch.get(from..to).unwrap_or(&[]) {
                    loudest = loudest.max(v);
                }
            }
            let h = height(loudest);
            if h > *level {
                *level = h;
            }
            if h >= self.peak[i] {
                self.peak[i] = h;
                self.peak_age[i] = Duration::ZERO;
            }
        }
        self.peak_level = self.peak_level.max(frame.peak[0].max(frame.peak[1]));
        self.rms_level = self.rms_level.max(frame.rms[0].max(frame.rms[1]));
    }

    /// Time passing with nothing new: bars fall at the release rate, peak
    /// marks after their hold.
    pub fn decay(&mut self, dt: Duration) {
        let s = dt.as_secs_f32();
        let fall = RELEASE_DB_PER_S * s / -FLOOR_DB;
        for level in &mut self.level {
            *level = (*level - fall).max(0.0);
        }
        let peak_fall = PEAK_FALL_DB_PER_S * s / -FLOOR_DB;
        for (peak, age) in self.peak.iter_mut().zip(self.peak_age.iter_mut()) {
            *age += dt;
            if *age > PEAK_HOLD {
                *peak = (*peak - peak_fall).max(0.0);
            }
        }
        // Levels fall like the bars do; a fresh hop lifts them again.
        let linear_fall = 1.0 - (-3.0 * s).exp();
        self.peak_level -= self.peak_level * linear_fall;
        self.rms_level -= self.rms_level * linear_fall;
    }

    pub fn reset(&mut self) {
        self.level.fill(0.0);
        self.peak.fill(0.0);
        self.peak_age.fill(Duration::ZERO);
        self.peak_level = 0.0;
        self.rms_level = 0.0;
    }
}

/// The bands bar `i` of `n` covers, as a half-open range into `bands`:
/// an even split when there are more bands than bars, one band each
/// (repeated) when there are fewer.
fn group(i: usize, n: usize, bands: usize) -> (usize, usize) {
    if bands >= n {
        (i * bands / n, ((i + 1) * bands / n).max(i * bands / n + 1))
    } else {
        let b = i * bands / n;
        (b, b + 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(bands: &[f32]) -> Frame {
        Frame {
            spectrum: [bands.to_vec(), bands.to_vec()],
            hold: [bands.to_vec(), bands.to_vec()],
            peak: [0.5, 0.25],
            rms: [0.1, 0.2],
            ..Frame::default()
        }
    }

    #[test]
    fn heights_follow_the_decibel_range() {
        assert_eq!(height(1.0), 1.0);
        assert_eq!(height(0.0), 0.0);
        assert!((height(0.001) - 0.0).abs() < 1e-6, "{}", height(0.001));
        assert!((height(0.1) - 2.0 / 3.0).abs() < 1e-6);
        assert_eq!(height(2.0), 1.0);
    }

    #[test]
    fn bands_group_into_bars_either_way() {
        assert_eq!(group(0, 4, 64), (0, 16));
        assert_eq!(group(3, 4, 64), (48, 64));
        assert_eq!(group(0, 3, 64), (0, 21));
        assert_eq!(group(2, 3, 64), (42, 64));
        assert_eq!(group(5, 8, 4), (2, 3));
        assert_eq!(group(7, 8, 4), (3, 4));
    }

    #[test]
    fn bars_rise_at_once_fall_in_time_and_hold_their_peaks() {
        let mut bars = Bars::new(4);
        let mut bands = vec![0.0f32; 64];
        bands[20] = 1.0; // falls in bar 1
        bars.feed(&frame(&bands), Duration::from_millis(50));
        assert_eq!(bars.levels()[1], 1.0);
        assert_eq!(bars.levels()[0], 0.0);
        assert_eq!(bars.peaks()[1], 1.0);
        assert!((bars.peak_level() - 0.5).abs() < 1e-6);
        assert!((bars.rms_level() - 0.2).abs() < 1e-6);
        // Half a second of silence: 45 dB down of 60, the peak still held.
        bars.decay(Duration::from_millis(500));
        assert!(
            (bars.levels()[1] - 0.25).abs() < 1e-5,
            "{}",
            bars.levels()[1]
        );
        assert_eq!(bars.peaks()[1], 1.0);
        // Past the hold the peak falls too.
        bars.decay(Duration::from_millis(500));
        assert!(bars.peaks()[1] < 1.0);
        assert_eq!(bars.levels()[1], 0.0);
        bars.reset();
        assert!(bars.peaks().iter().all(|&p| p == 0.0));
    }

    #[test]
    fn one_bit_frames_move_the_level_alone() {
        let mut bars = Bars::new(8);
        let f = Frame {
            one_bit: true,
            peak: [0.7, 0.7],
            rms: [0.3, 0.3],
            ..Frame::default()
        };
        bars.feed(&f, Duration::from_millis(50));
        assert!(bars.levels().iter().all(|&l| l == 0.0));
        assert!((bars.peak_level() - 0.7).abs() < 1e-6);
    }
}
