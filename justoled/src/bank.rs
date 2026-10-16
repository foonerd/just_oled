//! The spectrum bank: the analyser behind the spectrum pages. A port of
//! glass's `bank` crate (Just a Nerd), itself a port of the evo framework's
//! terminus audio plugin, with the FFT replaced by this crate's own and the
//! two channels sent through one transform.
//!
//! Pure compute, no I/O. Fed the last `window` samples of each channel on
//! the unit scale, it runs a real-input FFT per channel through a Hann
//! window, projects the magnitudes onto the bands asked for (up to 256
//! over 20 Hz to 20 kHz, under log, mel or linear spacing), scales them so
//! a full-scale sine inside a band reads 1.0, and keeps a peak hold per
//! band that falls with a quarter-second half-life. The frame also carries
//! an onset per band group.
//!
//! The capture advances by [`HOP`] samples between measurements while the
//! FFT looks at a longer window, so the cadence stays near 47 a second and
//! the resolution comes from the window. Log spacing is the ANSI/IEC S1.11
//! base-10 equal-ratio partition, the music-analyser convention. Adjacent
//! bands that would read the same FFT bins are told apart at construction:
//! the range is split when the window allows, else the bands share it with
//! triangular weights, so a dense bank never shows plateaus.
//!
//! Numerical code with explicit indexed loops over parallel per-band
//! arrays; the iterator forms the lint asks for would hide the shape.
#![allow(clippy::needless_range_loop)]

use crate::fft::{Complex, Fft};

/// Samples between measurements, per channel: about 47 a second at 48 kHz.
pub const HOP: usize = 1024;
/// The most bands a bank has.
pub const MAX_BINS: usize = 256;
/// The bank's lower edge, the audible band's.
pub const LOW_HZ: f32 = 20.0;
/// The bank's upper edge.
pub const HIGH_HZ: f32 = 20_000.0;
/// Windows a demand may name.
pub const WINDOWS: [usize; 4] = [2048, 4096, 8192, 16384];
/// The peak hold's half-life in seconds.
const HOLD_HALF_LIFE_S: f32 = 0.25;
/// Frames of spectral flux an onset is judged against.
const ONSET_WINDOW: usize = 16;
/// An onset fires when the flux exceeds the mean by this many deviations.
const ONSET_K: f32 = 1.8;

/// How the bands are spaced across the audible range.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Scale {
    /// Equal ratios: many bands for the bass, as every music analyser.
    #[default]
    Log,
    /// Equal mel steps: the perceptual bank.
    Mel,
    /// Equal hertz: the raw layout, for diagnostics.
    Linear,
}

impl Scale {
    /// The scale a configuration names, or none for a word not known.
    pub fn parse(word: &str) -> Option<Self> {
        match word.trim().to_ascii_lowercase().as_str() {
            "log" => Some(Self::Log),
            "mel" => Some(Self::Mel),
            "linear" => Some(Self::Linear),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Log => "log",
            Self::Mel => "mel",
            Self::Linear => "linear",
        }
    }

    /// The scale as one byte, for a header.
    pub fn byte(self) -> u8 {
        match self {
            Self::Log => 0,
            Self::Mel => 1,
            Self::Linear => 2,
        }
    }

    pub fn from_byte(byte: u8) -> Option<Self> {
        match byte {
            0 => Some(Self::Log),
            1 => Some(Self::Mel),
            2 => Some(Self::Linear),
            _ => None,
        }
    }
}

/// What a bank is asked to be: how many bands, for how many channels,
/// on which scale, from how long a window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Demand {
    pub bins: usize,
    pub channels: usize,
    pub scale: Scale,
    /// The FFT's length in samples; zero asks for the window the band
    /// count wants.
    pub window: usize,
}

impl Default for Demand {
    fn default() -> Self {
        Self::new(64, 2, Scale::Log)
    }
}

impl Demand {
    /// A demand with the window its band count wants.
    pub fn new(bins: usize, channels: usize, scale: Scale) -> Self {
        Self {
            bins,
            channels,
            scale,
            window: 0,
        }
        .clean()
    }

    /// The window a band count wants: fine enough for its lowest band to
    /// be told from its neighbour without paying for more.
    pub fn window_for(bins: usize) -> usize {
        match bins {
            0..=64 => 4096,
            65..=128 => 8192,
            _ => 16384,
        }
    }

    /// The demand within what a bank can be: 1 to 256 bands, one or two
    /// channels, a window from the list, longer than a hop.
    pub fn clean(self) -> Self {
        let bins = self.bins.clamp(1, MAX_BINS);
        let window = if self.window == 0 {
            Self::window_for(bins)
        } else {
            WINDOWS
                .iter()
                .copied()
                .find(|w| *w >= self.window)
                .unwrap_or(WINDOWS[WINDOWS.len() - 1])
        };
        Self {
            bins,
            channels: self.channels.clamp(1, 2),
            scale: self.scale,
            window,
        }
    }
}

/// Which band groups had an onset this hop.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Onsets {
    pub sub_bass: bool,
    pub bass: bool,
    pub mid: bool,
    pub high: bool,
}

impl Onsets {
    /// The four as bits, sub-bass lowest.
    pub fn bits(self) -> u8 {
        (self.sub_bass as u8)
            | (self.bass as u8) << 1
            | (self.mid as u8) << 2
            | (self.high as u8) << 3
    }

    pub fn from_bits(bits: u8) -> Self {
        Self {
            sub_bass: bits & 1 != 0,
            bass: bits & 2 != 0,
            mid: bits & 4 != 0,
            high: bits & 8 != 0,
        }
    }
}

/// One hop's bank: the bands per channel, channel-major (`bins` values
/// for the left or the mixed channel, then `bins` for the right when there
/// are two), the peak hold in the same layout, and the onsets.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Bank {
    pub bins: usize,
    pub channels: usize,
    pub magnitudes: Vec<f32>,
    pub hold: Vec<f32>,
    pub onsets: Onsets,
}

impl Bank {
    /// The bands of one channel; the mixed channel for a one-channel bank.
    pub fn channel(&self, ch: usize) -> &[f32] {
        let ch = ch.min(self.channels.saturating_sub(1));
        &self.magnitudes[ch * self.bins..(ch + 1) * self.bins]
    }

    /// The peak hold of one channel.
    pub fn channel_hold(&self, ch: usize) -> &[f32] {
        let ch = ch.min(self.channels.saturating_sub(1));
        &self.hold[ch * self.bins..(ch + 1) * self.bins]
    }
}

/// The bands' index ranges of the four groups an onset is judged in:
/// 20 to 60 Hz, 60 to 250, 250 to 2000, 2000 and up.
#[derive(Clone, Copy, Debug)]
struct Groups {
    sub_bass: (usize, usize),
    bass: (usize, usize),
    mid: (usize, usize),
    high: (usize, usize),
}

/// The analyser for one stream and one demand; a new demand or rate makes
/// a new one, the peak hold and the onset history starting again.
pub struct Analyser {
    demand: Demand,
    rate: u32,
    fft: Fft,
    /// Both channels packed, left real, right imaginary.
    work: Vec<Complex>,
    /// The one-sided spectra after the split, n/2 + 1 bins each.
    spectra: [Vec<Complex>; 2],
    hann: Vec<f32>,
    /// FFT bin range `[lo, hi)` per band, told apart at construction.
    ranges: Vec<(usize, usize)>,
    /// The amplitude weight per band, one where a band owns its range,
    /// triangular where bands share a starved one.
    weights: Vec<f32>,
    edges: Vec<(f32, f32)>,
    centres: Vec<f32>,
    hold: Vec<Vec<f32>>,
    /// Scratch for a hop: the bands per input channel, and their mix.
    per_channel: [Vec<f32>; 2],
    mixed: Vec<f32>,
    /// The hold's fall per hop, from the half-life and the hop's time.
    decay: f32,
    flux: [[f32; ONSET_WINDOW]; 4],
    flux_at: usize,
    last_group: [f32; 4],
    groups: Groups,
}

impl Analyser {
    /// For a stream at `rate`, as `demand` asks, the demand cleaned first.
    pub fn new(rate: u32, demand: Demand) -> Self {
        let demand = demand.clean();
        let window = demand.window;
        let centres = centres(demand.bins, demand.scale);
        let edges = edges(demand.bins, demand.scale);
        let (ranges, weights) = filterbank(&edges, &centres, rate, window);
        let hann: Vec<f32> = (0..window)
            .map(|i| {
                let phase = 2.0 * std::f32::consts::PI * i as f32 / (window - 1) as f32;
                0.5 - 0.5 * phase.cos()
            })
            .collect();
        let groups = groups(&centres, demand.bins);
        let hop_s = HOP as f32 / rate.max(1) as f32;
        Self {
            demand,
            rate,
            fft: Fft::new(window),
            work: vec![Complex::ZERO; window],
            spectra: [
                vec![Complex::ZERO; window / 2 + 1],
                vec![Complex::ZERO; window / 2 + 1],
            ],
            hann,
            ranges,
            weights,
            edges,
            centres,
            hold: vec![vec![0.0; demand.bins]; demand.channels],
            per_channel: [vec![0.0; demand.bins], vec![0.0; demand.bins]],
            mixed: vec![0.0; demand.bins],
            decay: 0.5f32.powf(hop_s / HOLD_HALF_LIFE_S),
            flux: [[0.0; ONSET_WINDOW]; 4],
            flux_at: 0,
            last_group: [0.0; 4],
            groups,
        }
    }

    pub fn demand(&self) -> Demand {
        self.demand
    }

    pub fn rate(&self) -> u32 {
        self.rate
    }

    /// The window in samples, what `process` takes of each channel.
    pub fn window(&self) -> usize {
        self.demand.window
    }

    /// Each band's edges in hertz, low to high, meeting end to end.
    pub fn edges(&self) -> &[(f32, f32)] {
        &self.edges
    }

    /// Each band's centre in hertz.
    pub fn centres(&self) -> &[f32] {
        &self.centres
    }

    /// Forget the stream so far: the hold and the onset history.
    pub fn reset(&mut self) {
        for hold in self.hold.iter_mut() {
            hold.fill(0.0);
        }
        self.flux = [[0.0; ONSET_WINDOW]; 4];
        self.flux_at = 0;
        self.last_group = [0.0; 4];
    }

    /// One hop: `channels[ch]` holds the last `window` samples of that
    /// channel, oldest first, on the unit scale; a mono stream gives the
    /// same slice twice. Shorter slices read as silence before them.
    pub fn process(&mut self, channels: [&[f32]; 2]) -> Bank {
        let mut bank = Bank::default();
        self.process_into(channels, &mut bank);
        bank
    }

    /// As `process`, into a bank the caller keeps: its vectors are reused,
    /// so a hop allocates nothing once they have their size.
    pub fn process_into(&mut self, channels: [&[f32]; 2], out: &mut Bank) {
        let window = self.demand.window;
        let bins = self.demand.bins;
        // Left as the real part, right as the imaginary: one transform for both.
        let lead = [
            window.saturating_sub(channels[0].len()),
            window.saturating_sub(channels[1].len()),
        ];
        for i in 0..window {
            let l = if i < lead[0] {
                0.0
            } else {
                channels[0][i - lead[0]]
            };
            let r = if i < lead[1] {
                0.0
            } else {
                channels[1][i - lead[1]]
            };
            self.work[i] = Complex::new(l * self.hann[i], r * self.hann[i]);
        }
        let [left, right] = &mut self.spectra;
        self.fft.split_real_pair(&mut self.work, left, right);
        // The power summed over the band's bins, then the root: the band's
        // energy is kept however many bins it spans. Four over the window
        // undoes the one-sided spectrum and the Hann window for a sine's
        // peak bin, and the window's main lobe spans three bins whose power
        // sums to one and a half times the peak's, so a full-scale sine
        // inside a band reads one.
        let scale = 4.0 / (window as f32 * 1.5f32.sqrt());
        let per_channel = &mut self.per_channel;
        for ch in 0..2 {
            let spectrum = &self.spectra[ch];
            for band in 0..bins {
                let (lo, hi) = self.ranges[band];
                per_channel[ch][band] = 0.0;
                if hi <= lo {
                    continue;
                }
                let mut power = 0.0f32;
                for k in lo..hi {
                    power += spectrum[k].norm_sqr();
                }
                let amplitude = power.sqrt() * self.weights[band] * scale;
                per_channel[ch][band] = amplitude.clamp(0.0, 1.0);
            }
        }
        for i in 0..bins {
            self.mixed[i] = (per_channel[0][i] + per_channel[1][i]) * 0.5;
        }
        let levels = group_levels(&self.mixed, self.groups);
        let onsets = self.onsets(levels);
        let out_channels = self.demand.channels;
        out.bins = bins;
        out.channels = out_channels;
        out.onsets = onsets;
        out.magnitudes.clear();
        out.magnitudes.resize(bins * out_channels, 0.0);
        out.hold.clear();
        out.hold.resize(bins * out_channels, 0.0);
        if out_channels == 2 {
            for ch in 0..2 {
                for i in 0..bins {
                    let level = self.per_channel[ch][i];
                    let kept = (self.hold[ch][i] * self.decay).max(level);
                    self.hold[ch][i] = kept;
                    out.magnitudes[ch * bins + i] = level;
                    out.hold[ch * bins + i] = kept;
                }
            }
        } else {
            for i in 0..bins {
                let level = self.mixed[i];
                let kept = (self.hold[0][i] * self.decay).max(level);
                self.hold[0][i] = kept;
                out.magnitudes[i] = level;
                out.hold[i] = kept;
            }
        }
    }

    /// An onset per group: the rise since the last hop against the recent
    /// rises' mean and spread, the hop itself left out of the history it
    /// is judged by.
    fn onsets(&mut self, levels: [f32; 4]) -> Onsets {
        let mut fired = [false; 4];
        for group in 0..4 {
            let flux = (levels[group] - self.last_group[group]).max(0.0);
            self.flux[group][self.flux_at] = flux;
            self.last_group[group] = levels[group];
            let history = self.flux[group]
                .iter()
                .enumerate()
                .filter(|(i, _)| *i != self.flux_at)
                .map(|(_, f)| *f);
            let count = (ONSET_WINDOW - 1) as f32;
            let mean = history.clone().sum::<f32>() / count;
            let variance = history.map(|f| (f - mean) * (f - mean)).sum::<f32>() / count;
            let threshold = mean + ONSET_K * variance.sqrt();
            fired[group] = flux > threshold && flux > 0.01;
        }
        self.flux_at = (self.flux_at + 1) % ONSET_WINDOW;
        Onsets {
            sub_bass: fired[0],
            bass: fired[1],
            mid: fired[2],
            high: fired[3],
        }
    }
}

fn hz_to_mel(hz: f32) -> f32 {
    2595.0 * (1.0 + hz / 700.0).log10()
}

fn mel_to_hz(mel: f32) -> f32 {
    700.0 * (10.0f32.powf(mel / 2595.0) - 1.0)
}

/// Where a fraction `t` of the way across the range falls in hertz.
fn across(t: f32, scale: Scale) -> f32 {
    match scale {
        Scale::Log => {
            let (lo, hi) = (LOW_HZ.log10(), HIGH_HZ.log10());
            10.0f32.powf(lo + (hi - lo) * t)
        }
        Scale::Mel => {
            let (lo, hi) = (hz_to_mel(LOW_HZ), hz_to_mel(HIGH_HZ));
            mel_to_hz(lo + (hi - lo) * t)
        }
        Scale::Linear => LOW_HZ + (HIGH_HZ - LOW_HZ) * t,
    }
}

/// Each band's centre in hertz: the middle of its cell on the scale.
pub fn centres(bins: usize, scale: Scale) -> Vec<f32> {
    let n = bins.max(1) as f32;
    (0..bins.max(1))
        .map(|i| across((i as f32 + 0.5) / n, scale))
        .collect()
}

/// Each band's edges in hertz, a partition of the range with the ends
/// pinned so float drift never walks past them.
pub fn edges(bins: usize, scale: Scale) -> Vec<(f32, f32)> {
    let bins = bins.max(1);
    let n = bins as f32;
    let edge = |i: usize| {
        if i == 0 {
            LOW_HZ
        } else if i == bins {
            HIGH_HZ
        } else {
            across(i as f32 / n, scale)
        }
    };
    (0..bins).map(|i| (edge(i), edge(i + 1))).collect()
}

/// The FFT bin range of each band and its weight. Bands that would read
/// the same bins are told apart: the range is split among them when it
/// holds enough bins, else they share it with triangular weights by how
/// near each band's centre lies to the bins, the nearest at full weight,
/// and a range with no bin at all lights only its first band.
fn filterbank(
    edges: &[(f32, f32)],
    centres: &[f32],
    rate: u32,
    window: usize,
) -> (Vec<(usize, usize)>, Vec<f32>) {
    let bin_width = rate as f32 / window as f32;
    let nyquist = window / 2;
    let n = edges.len();
    let mut ranges: Vec<(usize, usize)> = edges
        .iter()
        .map(|&(lo_hz, hi_hz)| {
            let lo = ((lo_hz / bin_width).floor() as usize).max(1);
            let hi = ((hi_hz / bin_width).ceil() as usize).min(nyquist);
            (lo, hi.max(lo))
        })
        .collect();
    let mut weights = vec![1.0f32; n];
    let mut i = 0;
    while i < n {
        let key = ranges[i];
        let mut j = i + 1;
        while j < n && ranges[j] == key {
            j += 1;
        }
        let run = j - i;
        let (lo, hi) = key;
        let span = hi.saturating_sub(lo);
        if run == 1 {
            if span == 0 {
                let one = lo.min(nyquist.saturating_sub(1)).max(1);
                ranges[i] = (one, one + 1);
            }
            i = j;
            continue;
        }
        if span >= run {
            for k in 0..run {
                let a = lo + span * k / run;
                let b = lo + span * (k + 1) / run;
                ranges[i + k] = (a, b.max(a + 1));
                weights[i + k] = 1.0;
            }
        } else if span >= 1 {
            let mut most = 0.0f32;
            for k in 0..run {
                ranges[i + k] = (lo, hi);
                let centre = centres[i + k];
                let mut best = 0.0f32;
                for bin in lo..hi {
                    let bin_centre = (bin as f32 + 0.5) * bin_width;
                    let distance = (centre - bin_centre).abs();
                    best = best.max(1.0 / (1.0 + distance / bin_width));
                }
                weights[i + k] = best.max(0.05);
                most = most.max(weights[i + k]);
            }
            if most > 0.0 {
                for k in 0..run {
                    weights[i + k] /= most;
                }
            }
        } else {
            let one = lo.min(nyquist.saturating_sub(1)).max(1);
            for k in 0..run {
                ranges[i + k] = (one, one + 1);
                weights[i + k] = if k == 0 { 1.0 } else { 0.0 };
            }
        }
        i = j;
    }
    (ranges, weights)
}

fn groups(centres: &[f32], bins: usize) -> Groups {
    let first_at_or_above =
        |hz: f32| -> usize { centres.iter().position(|&c| c >= hz).unwrap_or(bins) };
    let sub_bass = (first_at_or_above(LOW_HZ), first_at_or_above(60.0));
    let bass = (sub_bass.1, first_at_or_above(250.0));
    let mid = (bass.1, first_at_or_above(2_000.0));
    let high = (mid.1, bins);
    Groups {
        sub_bass,
        bass,
        mid,
        high,
    }
}

fn group_levels(mixed: &[f32], groups: Groups) -> [f32; 4] {
    let mean = |(lo, hi): (usize, usize)| -> f32 {
        if hi <= lo {
            return 0.0;
        }
        mixed[lo..hi].iter().sum::<f32>() / (hi - lo) as f32
    };
    [
        mean(groups.sub_bass),
        mean(groups.bass),
        mean(groups.mid),
        mean(groups.high),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn silence(n: usize) -> Vec<f32> {
        vec![0.0; n]
    }

    fn sine(hz: f32, rate: u32, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| (2.0 * std::f32::consts::PI * hz * i as f32 / rate as f32).sin())
            .collect()
    }

    fn argmax(values: &[f32]) -> usize {
        let mut best = 0;
        for (i, v) in values.iter().enumerate() {
            if *v > values[best] {
                best = i;
            }
        }
        best
    }

    #[test]
    fn a_demand_is_cleaned_and_its_window_follows_the_band_count() {
        assert_eq!(Demand::new(64, 2, Scale::Log).window, 4096);
        assert_eq!(Demand::new(128, 1, Scale::Mel).window, 8192);
        assert_eq!(Demand::new(256, 2, Scale::Log).window, 16384);
        let odd = Demand {
            bins: 999,
            channels: 7,
            scale: Scale::Linear,
            window: 3000,
        }
        .clean();
        assert_eq!((odd.bins, odd.channels, odd.window), (256, 2, 4096));
        assert_eq!(Scale::parse(" MEL "), Some(Scale::Mel));
        assert_eq!(Scale::parse("octave"), None);
        for scale in [Scale::Log, Scale::Mel, Scale::Linear] {
            assert_eq!(Scale::from_byte(scale.byte()), Some(scale));
        }
    }

    #[test]
    fn the_bank_has_the_demanded_shape() {
        for (bins, channels) in [(256, 2), (64, 1), (32, 2)] {
            let mut analyser = Analyser::new(48_000, Demand::new(bins, channels, Scale::Mel));
            let quiet = silence(analyser.window());
            let bank = analyser.process([&quiet, &quiet]);
            assert_eq!((bank.bins, bank.channels), (bins, channels));
            assert_eq!(bank.magnitudes.len(), bins * channels);
            assert_eq!(bank.hold.len(), bins * channels);
            assert!(bank.magnitudes.iter().all(|m| m.abs() < 1e-6));
            assert_eq!(bank.channel(1).len(), bins);
            assert_eq!(bank.channel_hold(0).len(), bins);
        }
    }

    #[test]
    fn a_tone_lands_in_the_band_that_holds_it_on_every_scale() {
        for scale in [Scale::Log, Scale::Mel, Scale::Linear] {
            for bins in [32, 64, 128, 256] {
                let mut analyser = Analyser::new(48_000, Demand::new(bins, 2, scale));
                let tone = sine(1_000.0, 48_000, analyser.window());
                let bank = analyser.process([&tone, &silence(analyser.window())]);
                let loudest = argmax(bank.channel(0));
                let (lo, hi) = analyser.edges()[loudest];
                // A tone on a band's edge lights the neighbour the window's
                // main lobe leans into: two FFT bins of slack.
                let lobe = 2.0 * 48_000.0 / analyser.window() as f32;
                assert!(
                    lo - lobe <= 1_000.0 && 1_000.0 <= hi + lobe,
                    "{scale:?} x {bins}: the loudest band {loudest} spans {lo} to {hi}"
                );
                assert!(
                    bank.channel(0)[loudest] > 0.5,
                    "{scale:?} x {bins}: a full-scale tone reads {}",
                    bank.channel(0)[loudest]
                );
                assert!(
                    bank.channel(1).iter().all(|m| *m < 1e-4),
                    "the silent right channel stays silent through the shared transform: {:?}",
                    bank.channel(1).iter().cloned().fold(0.0f32, f32::max)
                );
            }
        }
    }

    #[test]
    fn the_right_channel_is_read_as_well_as_the_left() {
        let mut analyser = Analyser::new(48_000, Demand::new(64, 2, Scale::Log));
        let window = analyser.window();
        let low = sine(100.0, 48_000, window);
        let high = sine(5_000.0, 48_000, window);
        let bank = analyser.process([&low, &high]);
        let (l, r) = (argmax(bank.channel(0)), argmax(bank.channel(1)));
        let (llo, lhi) = analyser.edges()[l];
        let (rlo, rhi) = analyser.edges()[r];
        // The window's main lobe leans into a neighbour: two FFT bins of slack.
        let lobe = 2.0 * 48_000.0 / window as f32;
        assert!(
            llo - lobe <= 100.0 && 100.0 <= lhi + lobe,
            "left loudest spans {llo}..{lhi}"
        );
        assert!(
            rlo - lobe <= 5_000.0 && 5_000.0 <= rhi + lobe,
            "right loudest spans {rlo}..{rhi}"
        );
        assert!(bank.channel(0)[l] > 0.5 && bank.channel(1)[r] > 0.5);
    }

    #[test]
    fn the_edges_partition_the_range_and_log_gives_the_bass_its_share() {
        for scale in [Scale::Log, Scale::Mel, Scale::Linear] {
            for bins in [32, 64, 128, 256] {
                let e = edges(bins, scale);
                assert_eq!(e.len(), bins);
                assert!((e[0].0 - LOW_HZ).abs() < 1e-2);
                assert!((e[bins - 1].1 - HIGH_HZ).abs() < 1e-2);
                for i in 0..bins - 1 {
                    assert!((e[i].1 - e[i + 1].0).abs() < 1e-2, "{scale:?} {bins} {i}");
                    assert!(e[i].0 < e[i].1);
                }
                assert_eq!(centres(bins, scale).len(), bins);
            }
        }
        let bass_share = |scale: Scale| {
            edges(64, scale)
                .iter()
                .filter(|(_, hi)| *hi <= 250.0)
                .count()
        };
        assert!(bass_share(Scale::Log) > bass_share(Scale::Mel));
        assert!(bass_share(Scale::Mel) > bass_share(Scale::Linear));
    }

    #[test]
    fn bands_that_share_bins_are_told_apart() {
        let analyser = Analyser::new(48_000, Demand::new(256, 2, Scale::Log));
        let (ranges, weights) = filterbank(analyser.edges(), analyser.centres(), 48_000, 16384);
        let clones = (1..256)
            .filter(|&i| ranges[i] == ranges[i - 1] && (weights[i] - weights[i - 1]).abs() < 1e-6)
            .count();
        assert_eq!(clones, 0, "no two adjacent bands read alike");
        assert!(weights.iter().all(|w| (0.0..=1.0).contains(w)));
        let wide = Analyser::new(48_000, Demand::new(64, 2, Scale::Log));
        let (ranges, weights) = filterbank(wide.edges(), wide.centres(), 48_000, 16384);
        assert!(ranges.windows(2).all(|w| w[0] != w[1]));
        assert!(weights.iter().all(|w| *w == 1.0));
    }

    #[test]
    fn the_hold_keeps_a_peak_and_falls_by_half_in_a_quarter_second() {
        let mut analyser = Analyser::new(48_000, Demand::new(64, 1, Scale::Log));
        let window = analyser.window();
        let tone = sine(1_000.0, 48_000, window);
        let loud = analyser.process([&tone, &tone]);
        let band = argmax(&loud.magnitudes);
        let peak = loud.hold[band];
        assert!((peak - loud.magnitudes[band]).abs() < 1e-6);
        let hops_per_quarter = (0.25 * 48_000.0 / HOP as f32).round() as usize;
        let quiet = silence(window);
        let mut last = loud;
        for _ in 0..hops_per_quarter {
            last = analyser.process([&quiet, &quiet]);
        }
        assert!(
            (last.hold[band] / peak - 0.5).abs() < 0.08,
            "after a quarter second the hold is at {}",
            last.hold[band] / peak
        );
        assert!(last.magnitudes[band] < 1e-6);
        analyser.reset();
        assert_eq!(analyser.process([&quiet, &quiet]).hold[band], 0.0);
    }

    #[test]
    fn an_onset_fires_on_a_rise_after_quiet() {
        let mut analyser = Analyser::new(48_000, Demand::new(64, 2, Scale::Log));
        let window = analyser.window();
        let quiet = silence(window);
        for _ in 0..ONSET_WINDOW + 2 {
            let bank = analyser.process([&quiet, &quiet]);
            assert_eq!(bank.onsets, Onsets::default());
        }
        let tone = sine(1_000.0, 48_000, window);
        let bank = analyser.process([&tone, &tone]);
        assert!(
            bank.onsets.mid,
            "a tone at 1 kHz after silence is an onset in the mids"
        );
        assert_eq!(Onsets::from_bits(bank.onsets.bits()), bank.onsets);
    }

    #[test]
    fn a_hop_allocates_nothing_once_the_bank_has_its_size() {
        let mut analyser = Analyser::new(44_100, Demand::new(32, 2, Scale::Log));
        let window = analyser.window();
        let tone = sine(440.0, 44_100, window);
        let mut bank = Bank::default();
        analyser.process_into([&tone, &tone], &mut bank);
        let (m, h) = (bank.magnitudes.capacity(), bank.hold.capacity());
        for _ in 0..5 {
            analyser.process_into([&tone, &tone], &mut bank);
        }
        assert_eq!((bank.magnitudes.capacity(), bank.hold.capacity()), (m, h));
    }
}
