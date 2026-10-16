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

#[cfg(feature = "display")]
pub mod catalog;
