//! The measurements: a hop at a time, the peak and RMS of each channel and
//! the bank of the last `window` samples of each channel. From glass's tap.

use crate::bank::{self, Bank, Demand};
use crate::ring::{Frame, MAX_CHANNELS};

pub struct Analyser {
    rate: u32,
    channels: usize,
    hop: usize,
    bank: bank::Analyser,
    history: [Vec<f32>; MAX_CHANNELS],
    /// The history in order, oldest first, for the bank.
    ordered: [Vec<f32>; MAX_CHANNELS],
    pos: usize,
    hop_fill: usize,
    frames: u64,
    peak: [f32; MAX_CHANNELS],
    square_sum: [f64; MAX_CHANNELS],
    measured: Bank,
    out: Frame,
}

impl Analyser {
    /// For a stream of `channels` at `rate`, measured as `demand` asks;
    /// `hop` how many frames between measurements, zero for the bank's.
    pub fn new(rate: u32, channels: u32, demand: Demand, hop: usize) -> Self {
        let bank = bank::Analyser::new(rate, demand);
        let window = bank.window();
        let hop = if hop == 0 {
            bank::HOP
        } else {
            hop.clamp(1, window)
        };
        let channels = (channels as usize).clamp(1, MAX_CHANNELS);
        let bins = bank.demand().bins;
        Self {
            rate,
            channels,
            hop,
            bank,
            history: [vec![0.0; window], vec![0.0; window]],
            ordered: [vec![0.0; window], vec![0.0; window]],
            pos: 0,
            hop_fill: 0,
            frames: 0,
            peak: [0.0; MAX_CHANNELS],
            square_sum: [0.0; MAX_CHANNELS],
            measured: Bank::default(),
            out: Frame {
                spectrum: [vec![0.0; bins], vec![0.0; bins]],
                hold: [vec![0.0; bins], vec![0.0; bins]],
                scale: demand.clean().scale,
                window: window as u32,
                ..Frame::default()
            },
        }
    }

    pub fn rate(&self) -> u32 {
        self.rate
    }
    pub fn channels(&self) -> usize {
        self.channels
    }
    /// The window the bank looks at, in samples.
    pub fn fft_size(&self) -> usize {
        self.bank.window()
    }
    pub fn hop(&self) -> usize {
        self.hop
    }
    /// The bands per channel.
    pub fn bins(&self) -> usize {
        self.bank.demand().bins
    }
    pub fn demand(&self) -> Demand {
        self.bank.demand()
    }

    /// Forget the stream so far; the next hop starts from silence.
    pub fn reset(&mut self) {
        for h in self.history.iter_mut() {
            h.fill(0.0);
        }
        self.pos = 0;
        self.hop_fill = 0;
        self.peak = [0.0; MAX_CHANNELS];
        self.square_sum = [0.0; MAX_CHANNELS];
        self.bank.reset();
    }

    /// Feed `n` frames, `chans[ch][i]` being frame `i` of channel `ch`; a
    /// mono stream gives one channel and is measured as both. `sink` is
    /// called once per completed hop with the frame and how many of the
    /// `n` frames had been taken in when the hop completed.
    pub fn feed(&mut self, chans: &[&[i16]], n: usize, sink: &mut dyn FnMut(&Frame, usize)) {
        let used = chans.len().min(self.channels).max(1);
        let window = self.history[0].len();
        for i in 0..n {
            for ch in 0..MAX_CHANNELS {
                let source = if ch < used { ch } else { 0 };
                let sample = chans
                    .get(source)
                    .and_then(|c| c.get(i))
                    .copied()
                    .unwrap_or(0);
                let s = sample as f32 / 32768.0;
                self.history[ch][self.pos] = s;
                let a = s.abs();
                if a > self.peak[ch] {
                    self.peak[ch] = a;
                }
                self.square_sum[ch] += (s as f64) * (s as f64);
            }
            self.pos = (self.pos + 1) % window;
            self.hop_fill += 1;
            self.frames += 1;
            if self.hop_fill >= self.hop {
                self.measure();
                sink(&self.out, i + 1);
                self.hop_fill = 0;
                self.peak = [0.0; MAX_CHANNELS];
                self.square_sum = [0.0; MAX_CHANNELS];
            }
        }
    }

    fn measure(&mut self) {
        let hop = self.hop as f64;
        self.out.frames = self.frames;
        for ch in 0..MAX_CHANNELS {
            self.out.peak[ch] = self.peak[ch].min(1.0);
            self.out.rms[ch] = (self.square_sum[ch] / hop).sqrt().min(1.0) as f32;
            // The last `window` samples, oldest first.
            let (tail, head) = self.history[ch].split_at(self.pos);
            self.ordered[ch][..head.len()].copy_from_slice(head);
            self.ordered[ch][head.len()..].copy_from_slice(tail);
        }
        self.bank
            .process_into([&self.ordered[0], &self.ordered[1]], &mut self.measured);
        for ch in 0..MAX_CHANNELS {
            self.out.spectrum[ch].copy_from_slice(self.measured.channel(ch));
            self.out.hold[ch].copy_from_slice(self.measured.channel_hold(ch));
        }
        self.out.onsets = self.measured.onsets.bits();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bank::Scale;

    fn sine(rate: u32, hz: f32, amplitude: f32, n: usize) -> Vec<i16> {
        (0..n)
            .map(|i| {
                (amplitude
                    * (2.0 * std::f32::consts::PI * hz * i as f32 / rate as f32).sin()
                    * 32767.0) as i16
            })
            .collect()
    }

    #[test]
    fn a_full_scale_sine_reads_one_in_its_band_and_its_peak_and_rms() {
        let rate = 48_000;
        let left = sine(rate, 1_000.0, 1.0, 8192);
        let right = sine(rate, 1_000.0, 0.5, 8192);
        let mut analyser = Analyser::new(rate, 2, Demand::new(64, 2, Scale::Log), 512);
        assert_eq!(analyser.fft_size(), 4096);
        assert_eq!(analyser.bins(), 64);
        let mut frames = Vec::new();
        let mut taken = Vec::new();
        analyser.feed(&[&left, &right], 8192, &mut |f, n| {
            frames.push(f.clone());
            taken.push(n);
        });
        assert_eq!(frames.len(), 16, "one hop every 512 frames");
        assert_eq!(taken[0], 512);
        assert_eq!(*taken.last().unwrap(), 8192);
        let last = frames.last().unwrap();
        assert!(
            last.peak[0] > 0.99 && last.peak[0] <= 1.0,
            "left peak {}",
            last.peak[0]
        );
        assert!(
            (last.peak[1] - 0.5).abs() < 0.01,
            "right peak {}",
            last.peak[1]
        );
        assert!(
            (last.rms[0] - 0.707).abs() < 0.01,
            "left rms {}",
            last.rms[0]
        );
        let loudest = (0..64)
            .max_by(|a, b| last.spectrum[0][*a].total_cmp(&last.spectrum[0][*b]))
            .unwrap();
        assert!(
            last.spectrum[0][loudest] > 0.9,
            "the tone's band reads {}",
            last.spectrum[0][loudest]
        );
        assert!(
            (last.spectrum[1][loudest] - 0.5).abs() < 0.1,
            "the right band reads {}",
            last.spectrum[1][loudest]
        );
        assert!(
            last.spectrum[0][5] < 0.01,
            "a band far below is quiet: {}",
            last.spectrum[0][5]
        );
        assert_eq!(last.hold[0][loudest], last.spectrum[0][loudest]);
        assert_eq!(last.frames, 8192);
        assert_eq!((last.scale, last.window), (Scale::Log, 4096));
        let mut mono = Analyser::new(rate, 1, Demand::new(64, 2, Scale::Log), 512);
        let mut got = Vec::new();
        mono.feed(&[&left], 1024, &mut |f, _| got.push(f.clone()));
        assert_eq!(got.last().unwrap().peak[0], got.last().unwrap().peak[1]);
        assert_eq!(
            got.last().unwrap().spectrum[0],
            got.last().unwrap().spectrum[1]
        );
    }

    #[test]
    fn silence_reads_zero_and_a_reset_forgets() {
        let mut analyser = Analyser::new(44_100, 2, Demand::new(32, 1, Scale::Log), 256);
        let loud = sine(44_100, 1000.0, 1.0, 4096);
        let mut frames = Vec::new();
        analyser.feed(&[&loud, &loud], 4096, &mut |f, _| frames.push(f.clone()));
        assert!(frames.last().unwrap().hold[0].iter().any(|h| *h > 0.5));
        analyser.reset();
        let quiet = vec![0i16; 4096];
        analyser.feed(&[&quiet, &quiet], 4096, &mut |f, _| frames.push(f.clone()));
        let last = frames.last().unwrap();
        assert_eq!(last.peak, [0.0, 0.0]);
        assert_eq!(last.rms, [0.0, 0.0]);
        assert!(last.spectrum[0].iter().all(|v| *v == 0.0));
        assert!(
            last.hold[0].iter().all(|v| *v == 0.0),
            "the hold forgot too"
        );
    }
}
