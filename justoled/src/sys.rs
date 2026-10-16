//! The system calls the display side needs, each through `libc` and the
//! kernel's uapi: an I2C device (`/dev/i2c-N`), an SPI device
//! (`/dev/spidevB.C`), output lines on a GPIO chip (`/dev/gpiochipN`, the
//! character device, uapi v2), and a program run through `posix_spawnp`.
//! The ioctl numbers are built as `<asm-generic/ioctl.h>` builds them,
//! which is what arm, aarch64 and x86 use; the struct layouts are the
//! headers' and their sizes are asserted in the tests.

use std::ffi::CString;
use std::fmt;
use std::io;
use std::os::fd::RawFd;
use std::path::Path;

/// An open file descriptor, closed when dropped.
struct Fd(RawFd);

impl Fd {
    fn open(path: &str, flags: libc::c_int) -> io::Result<Self> {
        let c = CString::new(path).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
        let fd = unsafe { libc::open(c.as_ptr(), flags | libc::O_CLOEXEC) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Fd(fd))
    }

    fn ioctl<T>(&self, request: libc::c_ulong, arg: *mut T) -> io::Result<()> {
        if unsafe { libc::ioctl(self.0, request as _, arg) } < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    fn write_all(&self, bytes: &[u8]) -> io::Result<()> {
        let n = unsafe { libc::write(self.0, bytes.as_ptr() as *const libc::c_void, bytes.len()) };
        if n < 0 {
            return Err(io::Error::last_os_error());
        }
        if n as usize != bytes.len() {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                format!("wrote {n} of {} bytes", bytes.len()),
            ));
        }
        Ok(())
    }
}

impl Drop for Fd {
    fn drop(&mut self) {
        unsafe { libc::close(self.0) };
    }
}

/// `_IOC(dir, type, nr, size)` as `<asm-generic/ioctl.h>` encodes it.
const fn ioc(dir: u32, ty: u8, nr: u8, size: usize) -> libc::c_ulong {
    ((dir << 30) | ((size as u32) << 16) | ((ty as u32) << 8) | nr as u32) as libc::c_ulong
}
const IOC_WRITE: u32 = 1;
const IOC_READ: u32 = 2;

// -----------------------------------------------------------------------------
// I2C
// -----------------------------------------------------------------------------

pub mod i2c {
    use super::*;

    const I2C_SLAVE: libc::c_ulong = 0x0703;

    /// A device at one address on one bus.
    pub struct Device {
        fd: Fd,
        bus: u8,
        addr: u8,
    }

    impl fmt::Debug for Device {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "i2c-{} 0x{:02X}", self.bus, self.addr)
        }
    }

    impl Device {
        /// Open `/dev/i2c-<bus>` and address `addr` (7-bit).
        pub fn open(bus: u8, addr: u8) -> io::Result<Self> {
            let fd = Fd::open(&format!("/dev/i2c-{bus}"), libc::O_RDWR)?;
            if unsafe { libc::ioctl(fd.0, I2C_SLAVE as _, addr as libc::c_ulong) } < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(Self { fd, bus, addr })
        }

        pub fn bus(&self) -> u8 {
            self.bus
        }

        pub fn addr(&self) -> u8 {
            self.addr
        }

        /// One write transaction: start, address, `bytes`, stop.
        pub fn write(&self, bytes: &[u8]) -> io::Result<()> {
            self.fd.write_all(bytes)
        }

        /// Whether a device answers at `addr` on `bus`: it acknowledges a
        /// one-byte read, as `i2cdetect` asks. A bus that cannot be opened
        /// reads as no device.
        pub fn present(bus: u8, addr: u8) -> bool {
            let Ok(dev) = Self::open(bus, addr) else {
                return false;
            };
            let mut byte = 0u8;
            let n = unsafe { libc::read(dev.fd.0, &mut byte as *mut u8 as *mut libc::c_void, 1) };
            n == 1
        }
    }

    /// Whether `/dev/i2c-<bus>` exists.
    pub fn bus_exists(bus: u8) -> bool {
        Path::new(&format!("/dev/i2c-{bus}")).exists()
    }

    /// The bus numbers that have a device node, in order.
    pub fn buses() -> Vec<u8> {
        (0..=99).filter(|b| bus_exists(*b)).collect()
    }
}

// -----------------------------------------------------------------------------
// SPI
// -----------------------------------------------------------------------------

pub mod spi {
    use super::*;

    const MAGIC: u8 = b'k';
    const WR_MODE: libc::c_ulong = ioc(IOC_WRITE, MAGIC, 1, 1);
    const WR_BITS_PER_WORD: libc::c_ulong = ioc(IOC_WRITE, MAGIC, 3, 1);
    const WR_MAX_SPEED_HZ: libc::c_ulong = ioc(IOC_WRITE, MAGIC, 4, 4);
    /// `SPI_IOC_MESSAGE(1)`.
    const MESSAGE_1: libc::c_ulong = ioc(IOC_WRITE, MAGIC, 0, size_of::<Transfer>());

    /// `struct spi_ioc_transfer`.
    #[repr(C)]
    #[derive(Default)]
    pub(super) struct Transfer {
        tx_buf: u64,
        rx_buf: u64,
        len: u32,
        speed_hz: u32,
        delay_usecs: u16,
        bits_per_word: u8,
        cs_change: u8,
        tx_nbits: u8,
        rx_nbits: u8,
        word_delay_usecs: u8,
        pad: u8,
    }

    /// One SPI device, write-only as every OLED is.
    pub struct Device {
        fd: Fd,
        bus: u8,
        cs: u8,
        speed_hz: u32,
        /// The most bytes one transfer may carry, the driver's `bufsiz`.
        chunk: usize,
    }

    impl fmt::Debug for Device {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "spidev{}.{} at {} Hz", self.bus, self.cs, self.speed_hz)
        }
    }

    impl Device {
        /// Open `/dev/spidev<bus>.<cs>` in SPI mode `mode` (0 to 3) at `speed_hz`.
        pub fn open(bus: u8, cs: u8, mode: u8, speed_hz: u32) -> io::Result<Self> {
            let fd = Fd::open(&format!("/dev/spidev{bus}.{cs}"), libc::O_RDWR)?;
            let mut m = mode & 3;
            fd.ioctl(WR_MODE, &mut m)?;
            let mut bits = 8u8;
            fd.ioctl(WR_BITS_PER_WORD, &mut bits)?;
            let mut hz = speed_hz;
            fd.ioctl(WR_MAX_SPEED_HZ, &mut hz)?;
            let chunk = std::fs::read_to_string("/sys/module/spidev/parameters/bufsiz")
                .ok()
                .and_then(|s| s.trim().parse::<usize>().ok())
                .unwrap_or(4096)
                .max(64);
            Ok(Self {
                fd,
                bus,
                cs,
                speed_hz,
                chunk,
            })
        }

        pub fn bus(&self) -> u8 {
            self.bus
        }

        pub fn cs(&self) -> u8 {
            self.cs
        }

        pub fn speed_hz(&self) -> u32 {
            self.speed_hz
        }

        /// Write `bytes`, in transfers no longer than the driver takes.
        pub fn write(&self, bytes: &[u8]) -> io::Result<()> {
            for part in bytes.chunks(self.chunk) {
                let mut t = Transfer {
                    tx_buf: part.as_ptr() as usize as u64,
                    len: part.len() as u32,
                    speed_hz: self.speed_hz,
                    bits_per_word: 8,
                    ..Transfer::default()
                };
                self.fd.ioctl(MESSAGE_1, &mut t)?;
            }
            Ok(())
        }
    }

    /// Whether `/dev/spidev<bus>.<cs>` exists.
    pub fn device_exists(bus: u8, cs: u8) -> bool {
        Path::new(&format!("/dev/spidev{bus}.{cs}")).exists()
    }

    /// Every `spidevB.C` present, as (bus, cs).
    pub fn devices() -> Vec<(u8, u8)> {
        let mut out = Vec::new();
        for entry in std::fs::read_dir("/dev").into_iter().flatten().flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if let Some(rest) = name.strip_prefix("spidev") {
                if let Some((b, c)) = rest.split_once('.') {
                    if let (Ok(b), Ok(c)) = (b.parse(), c.parse()) {
                        out.push((b, c));
                    }
                }
            }
        }
        out.sort_unstable();
        out
    }
}

// -----------------------------------------------------------------------------
// GPIO
// -----------------------------------------------------------------------------

pub mod gpio {
    use super::*;

    const MAGIC: u8 = 0xB4;
    const LINES_MAX: usize = 64;
    const NAME_SIZE: usize = 32;
    const ATTRS_MAX: usize = 10;
    const FLAG_OUTPUT: u64 = 1 << 3;
    const ATTR_ID_OUTPUT_VALUES: u32 = 2;

    /// `struct gpiochip_info`.
    #[repr(C)]
    pub(super) struct ChipInfo {
        name: [u8; NAME_SIZE],
        label: [u8; NAME_SIZE],
        lines: u32,
    }

    /// `struct gpio_v2_line_attribute`: the union is one u64 wide.
    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    pub(super) struct LineAttribute {
        id: u32,
        padding: u32,
        value: u64,
    }

    /// `struct gpio_v2_line_config_attribute`.
    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    pub(super) struct ConfigAttribute {
        attr: LineAttribute,
        mask: u64,
    }

    /// `struct gpio_v2_line_config`.
    #[repr(C)]
    pub(super) struct LineConfig {
        flags: u64,
        num_attrs: u32,
        padding: [u32; 5],
        attrs: [ConfigAttribute; ATTRS_MAX],
    }

    /// `struct gpio_v2_line_request`.
    #[repr(C)]
    pub(super) struct LineRequest {
        offsets: [u32; LINES_MAX],
        consumer: [u8; NAME_SIZE],
        config: LineConfig,
        num_lines: u32,
        event_buffer_size: u32,
        padding: [u32; 5],
        fd: i32,
    }

    /// `struct gpio_v2_line_values`.
    #[repr(C)]
    pub(super) struct LineValues {
        bits: u64,
        mask: u64,
    }

    const GET_CHIPINFO: libc::c_ulong = ioc(IOC_READ, MAGIC, 0x01, size_of::<ChipInfo>());
    const GET_LINE: libc::c_ulong =
        ioc(IOC_READ | IOC_WRITE, MAGIC, 0x07, size_of::<LineRequest>());
    const SET_VALUES: libc::c_ulong =
        ioc(IOC_READ | IOC_WRITE, MAGIC, 0x0F, size_of::<LineValues>());

    fn c_name(bytes: &[u8]) -> String {
        let end = bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len());
        String::from_utf8_lossy(&bytes[..end]).into_owned()
    }

    /// What a chip says of itself.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct Chip {
        pub path: String,
        pub name: String,
        pub label: String,
        pub lines: u32,
    }

    impl Chip {
        /// Ask `/dev/gpiochipN` who it is.
        pub fn info(path: &str) -> io::Result<Chip> {
            let fd = Fd::open(path, libc::O_RDONLY)?;
            let mut info = ChipInfo {
                name: [0; NAME_SIZE],
                label: [0; NAME_SIZE],
                lines: 0,
            };
            fd.ioctl(GET_CHIPINFO, &mut info)?;
            Ok(Chip {
                path: path.to_string(),
                name: c_name(&info.name),
                label: c_name(&info.label),
                lines: info.lines,
            })
        }

        /// Every chip under `/dev`, by number.
        pub fn all() -> Vec<Chip> {
            let mut out: Vec<Chip> = (0..=31u32)
                .filter_map(|n| {
                    let path = format!("/dev/gpiochip{n}");
                    // Symlinks (the Pi's gpiochip4 alias) would list one chip twice.
                    let real = std::fs::symlink_metadata(&path).ok()?;
                    if real.file_type().is_symlink() {
                        return None;
                    }
                    Chip::info(&path).ok()
                })
                .collect();
            out.sort_by(|a, b| a.path.cmp(&b.path));
            out
        }

        /// The chip behind the Raspberry Pi's 40-pin header: the SoC's pin
        /// controller (`pinctrl-bcm2835`, `pinctrl-bcm2711`, `pinctrl-rp1`),
        /// else the first chip there is.
        pub fn header() -> Option<Chip> {
            let all = Chip::all();
            all.iter()
                .find(|c| c.label.starts_with("pinctrl-"))
                .cloned()
                .or_else(|| all.into_iter().next())
        }
    }

    /// Output lines requested from one chip, driven together or one at a time.
    pub struct Outputs {
        fd: Fd,
        offsets: Vec<u32>,
    }

    impl fmt::Debug for Outputs {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "gpio outputs {:?}", self.offsets)
        }
    }

    impl Outputs {
        /// Request `offsets` on the chip at `path` as outputs, starting at
        /// `initial` (one bool per offset), under the consumer name given.
        pub fn request(
            path: &str,
            offsets: &[u32],
            initial: &[bool],
            consumer: &str,
        ) -> io::Result<Self> {
            if offsets.is_empty() || offsets.len() > LINES_MAX {
                return Err(io::Error::new(io::ErrorKind::InvalidInput, "1 to 64 lines"));
            }
            let chip = Fd::open(path, libc::O_RDWR)?;
            let mut req = LineRequest {
                offsets: [0; LINES_MAX],
                consumer: [0; NAME_SIZE],
                config: LineConfig {
                    flags: FLAG_OUTPUT,
                    num_attrs: 1,
                    padding: [0; 5],
                    attrs: [ConfigAttribute::default(); ATTRS_MAX],
                },
                num_lines: offsets.len() as u32,
                event_buffer_size: 0,
                padding: [0; 5],
                fd: -1,
            };
            req.offsets[..offsets.len()].copy_from_slice(offsets);
            let name = consumer.as_bytes();
            let n = name.len().min(NAME_SIZE - 1);
            req.consumer[..n].copy_from_slice(&name[..n]);
            let mut values = 0u64;
            for (i, &on) in initial.iter().enumerate().take(offsets.len()) {
                if on {
                    values |= 1 << i;
                }
            }
            req.config.attrs[0] = ConfigAttribute {
                attr: LineAttribute {
                    id: ATTR_ID_OUTPUT_VALUES,
                    padding: 0,
                    value: values,
                },
                mask: (1u64 << offsets.len()) - 1,
            };
            chip.ioctl(GET_LINE, &mut req)?;
            if req.fd < 0 {
                return Err(io::Error::other("the line request returned no descriptor"));
            }
            Ok(Self {
                fd: Fd(req.fd),
                offsets: offsets.to_vec(),
            })
        }

        pub fn offsets(&self) -> &[u32] {
            &self.offsets
        }

        /// Drive line `index` (into the offsets requested) high or low.
        pub fn set(&self, index: usize, high: bool) -> io::Result<()> {
            if index >= self.offsets.len() {
                return Err(io::Error::new(io::ErrorKind::InvalidInput, "no such line"));
            }
            let mut v = LineValues {
                bits: if high { 1 << index } else { 0 },
                mask: 1 << index,
            };
            self.fd.ioctl(SET_VALUES, &mut v)
        }
    }
}

// -----------------------------------------------------------------------------
// Programs
// -----------------------------------------------------------------------------

/// Run `argv[0]` with `argv` through `posix_spawnp`, wait for it, and give
/// back what it printed, or `None` when it did not finish in `timeout`.
/// The standard library's spawn refers to glibc 2.39's `pidfd_spawnp`,
/// which Volumio's 2.36 loader refuses, so this goes to libc directly.
pub fn run_with_timeout(argv: &[&str], timeout: std::time::Duration) -> Option<String> {
    use std::time::Instant;
    let c_args: Vec<CString> = argv.iter().filter_map(|a| CString::new(*a).ok()).collect();
    if c_args.len() != argv.len() || c_args.is_empty() {
        return None;
    }
    let mut ptrs: Vec<*mut libc::c_char> = c_args
        .iter()
        .map(|a| a.as_ptr() as *mut libc::c_char)
        .collect();
    ptrs.push(std::ptr::null_mut());
    let mut pipe = [0 as libc::c_int; 2];
    if unsafe { libc::pipe(pipe.as_mut_ptr()) } != 0 {
        return None;
    }
    let (read_end, write_end) = (Fd(pipe[0]), pipe[1]);
    let mut actions: libc::posix_spawn_file_actions_t = unsafe { std::mem::zeroed() };
    let mut pid: libc::pid_t = 0;
    let spawned = unsafe {
        libc::posix_spawn_file_actions_init(&mut actions);
        libc::posix_spawn_file_actions_adddup2(&mut actions, write_end, 1);
        libc::posix_spawn_file_actions_addclose(&mut actions, write_end);
        libc::posix_spawn_file_actions_addclose(&mut actions, read_end.0);
        let rc = libc::posix_spawnp(
            &mut pid,
            ptrs[0],
            &actions,
            std::ptr::null(),
            ptrs.as_ptr(),
            environ_ptr(),
        );
        libc::posix_spawn_file_actions_destroy(&mut actions);
        libc::close(write_end);
        rc == 0
    };
    if !spawned {
        return None;
    }
    // Non-blocking reads while the child runs, within the timeout.
    unsafe {
        let flags = libc::fcntl(read_end.0, libc::F_GETFL);
        libc::fcntl(read_end.0, libc::F_SETFL, flags | libc::O_NONBLOCK);
    }
    let started = Instant::now();
    let mut out = Vec::new();
    let mut buf = [0u8; 1024];
    let mut done = false;
    while started.elapsed() < timeout {
        let n = unsafe { libc::read(read_end.0, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
        if n > 0 {
            out.extend_from_slice(&buf[..n as usize]);
            continue;
        }
        if n == 0 {
            done = true;
            break;
        }
        let mut fds = libc::pollfd {
            fd: read_end.0,
            events: libc::POLLIN,
            revents: 0,
        };
        unsafe { libc::poll(&mut fds, 1, 20) };
    }
    let mut status: libc::c_int = 0;
    if done {
        unsafe { libc::waitpid(pid, &mut status, 0) };
        Some(String::from_utf8_lossy(&out).into_owned())
    } else {
        unsafe {
            libc::kill(pid, libc::SIGKILL);
            libc::waitpid(pid, &mut status, 0);
        }
        None
    }
}

extern "C" {
    static environ: *const *mut libc::c_char;
}

/// The process environment, for the spawned program.
fn environ_ptr() -> *const *mut libc::c_char {
    unsafe { environ }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ioctl_numbers_and_struct_sizes_are_the_headers() {
        // From <linux/spi/spidev.h> and <linux/gpio.h> as read on the build host.
        assert_eq!(size_of::<spi::Transfer>(), 32);
        assert_eq!(ioc(IOC_WRITE, b'k', 1, 1), 0x4001_6b01);
        assert_eq!(ioc(IOC_WRITE, b'k', 4, 4), 0x4004_6b04);
        assert_eq!(ioc(IOC_WRITE, b'k', 0, 32), 0x4020_6b00);
        assert_eq!(size_of::<gpio::ChipInfo>(), 68);
        assert_eq!(size_of::<gpio::LineAttribute>(), 16);
        assert_eq!(size_of::<gpio::ConfigAttribute>(), 24);
        assert_eq!(size_of::<gpio::LineConfig>(), 272);
        assert_eq!(size_of::<gpio::LineRequest>(), 592);
        assert_eq!(size_of::<gpio::LineValues>(), 16);
        assert_eq!(ioc(IOC_READ, 0xB4, 0x01, 68), 0x8044_b401);
        assert_eq!(ioc(IOC_READ | IOC_WRITE, 0xB4, 0x07, 592), 0xc250_b407);
        assert_eq!(ioc(IOC_READ | IOC_WRITE, 0xB4, 0x0F, 16), 0xc010_b40f);
    }

    #[test]
    fn a_program_s_output_comes_back_and_a_hang_is_cut_short() {
        let out = run_with_timeout(
            &["sh", "-c", "echo hello; echo there"],
            std::time::Duration::from_secs(5),
        );
        assert_eq!(out.as_deref(), Some("hello\nthere\n"));
        let none = run_with_timeout(
            &["sh", "-c", "sleep 5"],
            std::time::Duration::from_millis(100),
        );
        assert_eq!(none, None);
        // glibc reports a missing program from posix_spawnp itself; an older one would exec nothing.
        let missing =
            run_with_timeout(&["/nonexistent/program"], std::time::Duration::from_secs(1));
        assert!(matches!(missing.as_deref(), None | Some("")));
    }

    #[test]
    fn device_listing_does_not_panic_without_hardware() {
        let _ = i2c::buses();
        let _ = spi::devices();
        let _ = gpio::Chip::all();
        assert!(!i2c::bus_exists(99));
    }
}
