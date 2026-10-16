//! A complex FFT for the spectrum bank: iterative radix-2, in place, for
//! power-of-two sizes, with the twiddles computed once per size. Forward
//! transform only, unnormalised, as the bank wants it. Two real channels
//! go through one transform packed as real and imaginary parts and are
//! split afterwards.

use std::ops::{Add, Mul, Sub};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Complex {
    pub re: f32,
    pub im: f32,
}

impl Complex {
    pub const ZERO: Complex = Complex { re: 0.0, im: 0.0 };

    pub fn new(re: f32, im: f32) -> Self {
        Complex { re, im }
    }

    pub fn conj(self) -> Self {
        Complex {
            re: self.re,
            im: -self.im,
        }
    }

    /// |z|², without the square root.
    pub fn norm_sqr(self) -> f32 {
        self.re * self.re + self.im * self.im
    }

    pub fn norm(self) -> f32 {
        self.norm_sqr().sqrt()
    }
}

impl Add for Complex {
    type Output = Complex;
    fn add(self, o: Complex) -> Complex {
        Complex {
            re: self.re + o.re,
            im: self.im + o.im,
        }
    }
}

impl Sub for Complex {
    type Output = Complex;
    fn sub(self, o: Complex) -> Complex {
        Complex {
            re: self.re - o.re,
            im: self.im - o.im,
        }
    }
}

impl Mul for Complex {
    type Output = Complex;
    fn mul(self, o: Complex) -> Complex {
        Complex {
            re: self.re * o.re - self.im * o.im,
            im: self.re * o.im + self.im * o.re,
        }
    }
}

/// A forward transform of one fixed size.
pub struct Fft {
    n: usize,
    /// e^(-2πik/n) for k in 0..n/2.
    twiddles: Vec<Complex>,
    /// The bit-reversed index of each position.
    reversed: Vec<u32>,
}

impl Fft {
    /// `n` must be a power of two of at least 2.
    pub fn new(n: usize) -> Self {
        assert!(
            n >= 2 && n.is_power_of_two(),
            "fft size {n} is not a power of two"
        );
        let twiddles = (0..n / 2)
            .map(|k| {
                let a = -2.0 * std::f64::consts::PI * k as f64 / n as f64;
                Complex::new(a.cos() as f32, a.sin() as f32)
            })
            .collect();
        let bits = n.trailing_zeros();
        let reversed = (0..n as u32)
            .map(|i| i.reverse_bits() >> (32 - bits))
            .collect();
        Fft {
            n,
            twiddles,
            reversed,
        }
    }

    pub fn len(&self) -> usize {
        self.n
    }

    pub fn is_empty(&self) -> bool {
        false
    }

    /// Transform `buf` in place; `buf.len()` must be the size.
    pub fn process(&self, buf: &mut [Complex]) {
        assert_eq!(buf.len(), self.n, "fft buffer length");
        for (i, &r) in self.reversed.iter().enumerate() {
            let r = r as usize;
            if r > i {
                buf.swap(i, r);
            }
        }
        let mut half = 1;
        while half < self.n {
            let stride = self.n / (half * 2);
            for block in buf.chunks_exact_mut(half * 2) {
                let (lo, hi) = block.split_at_mut(half);
                for k in 0..half {
                    let t = self.twiddles[k * stride] * hi[k];
                    hi[k] = lo[k] - t;
                    lo[k] = lo[k] + t;
                }
            }
            half *= 2;
        }
    }

    /// The spectra of two real signals from one transform: `buf` holds
    /// `a` as the real parts and `b` as the imaginary parts on entry, and
    /// on return `a_out[k]` and `b_out[k]` for k in 0..=n/2 are their
    /// transforms. The outputs must hold n/2 + 1 values each.
    pub fn split_real_pair(
        &self,
        buf: &mut [Complex],
        a_out: &mut [Complex],
        b_out: &mut [Complex],
    ) {
        self.process(buf);
        let n = self.n;
        let half = n / 2;
        assert!(
            a_out.len() > half && b_out.len() > half,
            "split outputs too short"
        );
        for k in 0..=half {
            let z = buf[k];
            let w = buf[(n - k) % n].conj();
            // A = (Z[k] + conj Z[n-k]) / 2, B = (Z[k] - conj Z[n-k]) / 2i.
            let sum = z + w;
            let diff = z - w;
            a_out[k] = Complex::new(sum.re * 0.5, sum.im * 0.5);
            b_out[k] = Complex::new(diff.im * 0.5, -diff.re * 0.5);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn naive(x: &[Complex]) -> Vec<Complex> {
        let n = x.len();
        (0..n)
            .map(|k| {
                let mut acc = (0.0f64, 0.0f64);
                for (j, v) in x.iter().enumerate() {
                    let a = -2.0 * std::f64::consts::PI * (k * j) as f64 / n as f64;
                    let (s, c) = a.sin_cos();
                    acc.0 += v.re as f64 * c - v.im as f64 * s;
                    acc.1 += v.re as f64 * s + v.im as f64 * c;
                }
                Complex::new(acc.0 as f32, acc.1 as f32)
            })
            .collect()
    }

    fn close(a: &[Complex], b: &[Complex], tol: f32) {
        assert_eq!(a.len(), b.len());
        for (i, (x, y)) in a.iter().zip(b).enumerate() {
            assert!((*x - *y).norm() <= tol, "bin {i}: {x:?} vs {y:?}");
        }
    }

    fn signal(n: usize) -> Vec<Complex> {
        // Deterministic, aperiodic, both parts used.
        (0..n)
            .map(|i| {
                let t = i as f32;
                Complex::new(
                    (t * 0.37).sin() + 0.3 * (t * 1.9).cos(),
                    0.5 * (t * 0.11).cos() - (t * 2.3).sin() * 0.2,
                )
            })
            .collect()
    }

    #[test]
    fn matches_the_direct_transform_at_every_size_the_bank_uses() {
        for n in [2usize, 4, 8, 64, 256, 2048, 4096] {
            let x = signal(n);
            let mut buf = x.clone();
            Fft::new(n).process(&mut buf);
            close(&buf, &naive(&x), 2e-3 * n as f32);
        }
    }

    #[test]
    fn a_pure_sine_lands_in_its_bin_with_the_right_height() {
        let n = 4096;
        let bin = 300;
        let x: Vec<Complex> = (0..n)
            .map(|i| {
                Complex::new(
                    (2.0 * std::f32::consts::PI * bin as f32 * i as f32 / n as f32).sin(),
                    0.0,
                )
            })
            .collect();
        let mut buf = x;
        Fft::new(n).process(&mut buf);
        // A real sine of amplitude 1 puts n/2 in bins ±k and nothing elsewhere.
        assert!(
            (buf[bin].norm() - n as f32 / 2.0).abs() < 0.5,
            "{}",
            buf[bin].norm()
        );
        assert!((buf[n - bin].norm() - n as f32 / 2.0).abs() < 0.5);
        let leak: f32 = (0..n)
            .filter(|&k| k != bin && k != n - bin)
            .map(|k| buf[k].norm())
            .fold(0.0, f32::max);
        assert!(leak < 0.05, "leak {leak}");
    }

    #[test]
    fn two_real_channels_split_from_one_transform() {
        let n = 1024;
        let a: Vec<f32> = (0..n).map(|i| ((i as f32) * 0.05).sin()).collect();
        let b: Vec<f32> = (0..n).map(|i| ((i as f32) * 0.21).cos() * 0.7).collect();
        let mut packed: Vec<Complex> = a
            .iter()
            .zip(&b)
            .map(|(&x, &y)| Complex::new(x, y))
            .collect();
        let mut a_out = vec![Complex::ZERO; n / 2 + 1];
        let mut b_out = vec![Complex::ZERO; n / 2 + 1];
        let fft = Fft::new(n);
        fft.split_real_pair(&mut packed, &mut a_out, &mut b_out);
        let a_ref = naive(&a.iter().map(|&x| Complex::new(x, 0.0)).collect::<Vec<_>>());
        let b_ref = naive(&b.iter().map(|&x| Complex::new(x, 0.0)).collect::<Vec<_>>());
        close(&a_out, &a_ref[..=n / 2], 1e-2);
        close(&b_out, &b_ref[..=n / 2], 1e-2);
    }

    #[test]
    #[should_panic(expected = "not a power of two")]
    fn refuses_a_size_that_is_not_a_power_of_two() {
        Fft::new(1000);
    }
}
