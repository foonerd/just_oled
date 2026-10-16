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
/// The gain the user may add or take, in decibels, for quiet or loud masters.
pub const GAIN_DB_RANGE: i32 = 12;

#[derive(Clone, Debug)]
pub struct Bars {
    /// Linear factor on every amplitude before the floor mapping.
    gain: f32,
    /// Bar heights 0..=1, the louder channel.
    level: Vec<f32>,
    /// Peak marks 0..=1.
    peak: Vec<f32>,
    /// Time since each peak was set.
    peak_age: Vec<Duration>,
    /// The same per channel, for a stereo picture.
    level_ch: [Vec<f32>; 2],
    peak_ch: [Vec<f32>; 2],
    peak_ch_age: [Vec<Duration>; 2],
    /// Peak and RMS of the louder channel, 0..=1.
    peak_level: f32,
    rms_level: f32,
    /// Peak and RMS per channel, 0..=1, for the L and R meters.
    peak_lr: [f32; 2],
    rms_lr: [f32; 2],
    peak_lr_age: [Duration; 2],
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
            gain: 1.0,
            level: vec![0.0; count],
            peak: vec![0.0; count],
            peak_age: vec![Duration::ZERO; count],
            level_ch: [vec![0.0; count], vec![0.0; count]],
            peak_ch: [vec![0.0; count], vec![0.0; count]],
            peak_ch_age: [vec![Duration::ZERO; count], vec![Duration::ZERO; count]],
            peak_level: 0.0,
            rms_level: 0.0,
            peak_lr: [0.0; 2],
            rms_lr: [0.0; 2],
            peak_lr_age: [Duration::ZERO; 2],
        }
    }

    pub fn count(&self) -> usize {
        self.level.len()
    }

    /// Gain in whole decibels, within `GAIN_DB_RANGE` either way.
    pub fn with_gain_db(mut self, db: i32) -> Bars {
        let db = db.clamp(-GAIN_DB_RANGE, GAIN_DB_RANGE);
        self.gain = 10f32.powf(db as f32 / 20.0);
        self
    }

    /// An amplitude to a bar height, through the gain.
    pub fn height(&self, amplitude: f32) -> f32 {
        height(amplitude * self.gain)
    }

    pub fn levels(&self) -> &[f32] {
        &self.level
    }

    pub fn peaks(&self) -> &[f32] {
        &self.peak
    }

    /// One channel's bar heights.
    pub fn levels_ch(&self, ch: usize) -> &[f32] {
        &self.level_ch[ch.min(1)]
    }

    pub fn peaks_ch(&self, ch: usize) -> &[f32] {
        &self.peak_ch[ch.min(1)]
    }

    /// The louder channel's peak, 0..=1 of full scale.
    pub fn peak_level(&self) -> f32 {
        self.peak_level
    }

    pub fn rms_level(&self) -> f32 {
        self.rms_level
    }

    /// Left and right RMS, 0..=1 of full scale, with the release applied.
    pub fn rms_lr(&self) -> [f32; 2] {
        self.rms_lr
    }

    /// Left and right peak marks, held then falling.
    pub fn peak_lr(&self) -> [f32; 2] {
        self.peak_lr
    }

    /// A hop's bank into the bars: each bar takes the loudest band of its
    /// group, rising at once and never below where the release would put it.
    pub fn feed(&mut self, frame: &Frame, dt: Duration) {
        self.decay(dt);
        for ch in 0..2 {
            let rms = self.height(frame.rms[ch]);
            let peak = self.height(frame.peak[ch]);
            if rms > self.rms_lr[ch] {
                self.rms_lr[ch] = rms;
            }
            if peak >= self.peak_lr[ch] {
                self.peak_lr[ch] = peak;
                self.peak_lr_age[ch] = Duration::ZERO;
            }
        }
        let bands = frame.spectrum[0].len().max(frame.spectrum[1].len());
        if bands == 0 {
            // One-bit audio: no bank; the level alone moves.
            self.peak_level = self.peak_level.max(frame.peak[0].max(frame.peak[1]));
            self.rms_level = self.rms_level.max(frame.rms[0].max(frame.rms[1]));
            return;
        }
        let n = self.level.len();
        for i in 0..n {
            let (from, to) = group(i, n, bands);
            let mut loudest = 0.0f32;
            for ch in 0..2 {
                let mut ch_loudest = 0.0f32;
                for &v in frame.spectrum[ch].get(from..to).unwrap_or(&[]) {
                    ch_loudest = ch_loudest.max(v);
                }
                let h = self.height(ch_loudest);
                if h > self.level_ch[ch][i] {
                    self.level_ch[ch][i] = h;
                }
                if h >= self.peak_ch[ch][i] {
                    self.peak_ch[ch][i] = h;
                    self.peak_ch_age[ch][i] = Duration::ZERO;
                }
                loudest = loudest.max(ch_loudest);
            }
            let h = self.height(loudest);
            if h > self.level[i] {
                self.level[i] = h;
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
        for ch in 0..2 {
            for level in &mut self.level_ch[ch] {
                *level = (*level - fall).max(0.0);
            }
            for (peak, age) in self.peak_ch[ch]
                .iter_mut()
                .zip(self.peak_ch_age[ch].iter_mut())
            {
                *age += dt;
                if *age > PEAK_HOLD {
                    *peak = (*peak - peak_fall).max(0.0);
                }
            }
        }
        // Levels fall like the bars do; a fresh hop lifts them again.
        let linear_fall = 1.0 - (-3.0 * s).exp();
        self.peak_level -= self.peak_level * linear_fall;
        self.rms_level -= self.rms_level * linear_fall;
        for ch in 0..2 {
            self.rms_lr[ch] = (self.rms_lr[ch] - fall).max(0.0);
            self.peak_lr_age[ch] += dt;
            if self.peak_lr_age[ch] > PEAK_HOLD {
                self.peak_lr[ch] = (self.peak_lr[ch] - peak_fall).max(0.0);
            }
        }
    }

    pub fn reset(&mut self) {
        self.level.fill(0.0);
        self.peak.fill(0.0);
        self.peak_age.fill(Duration::ZERO);
        for ch in 0..2 {
            self.level_ch[ch].fill(0.0);
            self.peak_ch[ch].fill(0.0);
            self.peak_ch_age[ch].fill(Duration::ZERO);
        }
        self.peak_level = 0.0;
        self.rms_level = 0.0;
        self.peak_lr = [0.0; 2];
        self.rms_lr = [0.0; 2];
        self.peak_lr_age = [Duration::ZERO; 2];
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
    fn the_channels_are_kept_apart_and_folded() {
        let mut bars = Bars::new(4);
        let mut left = vec![0.0f32; 64];
        left[5] = 1.0; // bar 0
        let mut right = vec![0.0f32; 64];
        right[60] = 0.1; // bar 3, -20 dB
        let f = Frame {
            spectrum: [left, right],
            ..Frame::default()
        };
        bars.feed(&f, Duration::from_millis(50));
        assert_eq!(bars.levels_ch(0)[0], 1.0);
        assert_eq!(bars.levels_ch(1)[0], 0.0);
        assert!((bars.levels_ch(1)[3] - 2.0 / 3.0).abs() < 1e-6);
        assert_eq!(bars.levels()[0], 1.0, "folded takes the louder");
        assert!((bars.levels()[3] - 2.0 / 3.0).abs() < 1e-6);
        bars.reset();
        assert!(bars.levels_ch(0).iter().all(|&l| l == 0.0));
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
        // Per channel, in decibels over the floor: 0.5 is -6 dB, 0.1 is -20 dB.
        assert!((bars.peak_lr()[0] - height(0.5)).abs() < 1e-6);
        assert!((bars.rms_lr()[1] - height(0.2)).abs() < 1e-6);
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
    fn the_gain_shifts_every_height_in_decibels_and_stops_at_the_range() {
        let mut bands = vec![0.0f32; 64];
        bands[20] = 0.5; // -6 dB
        let mut plain = Bars::new(4);
        plain.feed(&frame(&bands), Duration::from_millis(50));
        assert!((plain.levels()[1] - 0.9).abs() < 1e-3);
        // +6 dB lifts a -6 dB band to the top; the level meters follow.
        let mut lifted = Bars::new(4).with_gain_db(6);
        lifted.feed(&frame(&bands), Duration::from_millis(50));
        assert!(
            (lifted.levels()[1] - 1.0).abs() < 1e-3,
            "{}",
            lifted.levels()[1]
        );
        assert!((lifted.peak_lr()[0] - 1.0).abs() < 1e-3);
        assert!((lifted.height(0.2) - height(0.4)).abs() < 1e-3);
        // -12 dB puts the same band at -18 dB: 42 of 60.
        let mut cut = Bars::new(4).with_gain_db(-12);
        cut.feed(&frame(&bands), Duration::from_millis(50));
        assert!((cut.levels()[1] - 0.7).abs() < 1e-3, "{}", cut.levels()[1]);
        // Beyond the range is the range: 40 asks, 12 is given.
        let wild = Bars::new(4).with_gain_db(40);
        assert_eq!(wild.height(0.3), 1.0);
        assert!((wild.height(0.1) - height(0.1 * 10f32.powf(0.6))).abs() < 1e-6);
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
