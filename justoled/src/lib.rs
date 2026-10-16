//! Just OLED: one library behind two artefacts.
//!
//! With the `tap` feature: what the ALSA tap measures, peak and RMS per
//! channel and a bank of spectrum bands, and the ring under `/dev/shm` it
//! shares them through. With the `display` feature: the catalogue of OLED
//! controllers and panels, the I2C and SPI transports, the framebuffers in
//! each controller's pixel format, and the pages the daemon draws.
//!
//! The only external dependency is `libc`, for the system call ABI of each
//! target; everything else, the JSON reader, the FFT, the ioctls, the fonts,
//! is in this crate.

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub mod json;

// The ring is the contract between the tap and the daemon, so it and the
// bank's shape (which the ring's header carries) come with either feature.
#[cfg(any(feature = "tap", feature = "display"))]
pub mod fft;

#[cfg(any(feature = "tap", feature = "display"))]
pub mod bank;

#[cfg(any(feature = "tap", feature = "display"))]
pub mod ring;

#[cfg(feature = "tap")]
pub mod analysis;
#[cfg(feature = "tap")]
pub mod dop;
#[cfg(feature = "tap")]
pub mod dsd;
#[cfg(feature = "tap")]
pub mod measure;
#[cfg(feature = "tap")]
pub mod relay;
#[cfg(feature = "tap")]
pub mod sample;

#[cfg(feature = "display")]
pub mod catalog;

#[cfg(feature = "display")]
pub mod canvas;

#[cfg(feature = "display")]
pub mod font;

#[cfg(feature = "display")]
pub mod oled;

#[cfg(feature = "display")]
pub mod sys;
