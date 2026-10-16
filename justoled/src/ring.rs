//! The shared ring: a file under `/dev/shm` with a header and a ring of
//! slots, written by the tap and read by the display. One file per writing
//! process, so two players loaded with the tap never write over each other;
//! the reader takes the one written most recently. The design is glass's
//! ring; the layout is this project's own.
//!
//! Layout, little-endian, all offsets fixed by the header:
//!
//! - header, 256 bytes: magic `JUSTOLED`, version, header size, sample
//!   rate, channels, FFT size, bins per channel, slot count, slot size,
//!   writer pid, hop, the bank's scale, then the sequence of the last slot
//!   published and the time it was published, both written last with
//!   release ordering.
//! - slots: sequence, time in nanoseconds on the monotonic clock, frames
//!   seen so far, peak per channel, RMS per channel, the bank per channel,
//!   the hold per channel, the sequence again, then the flags. A reader
//!   whose two sequences disagree caught a write in flight and tries again.

use std::ffi::CString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use crate::bank::{self, Scale};

pub const MAGIC: &[u8; 8] = b"JUSTOLED";
pub const VERSION: u32 = 1;
pub const DIR: &str = "/dev/shm";
pub const PREFIX: &str = "justoled.";
pub const MAX_CHANNELS: usize = 2;
pub const HEADER_BYTES: usize = 256;
/// A ring written longer ago than this is a leftover of a player that stopped.
pub const LIVE_NS: u64 = 3_000_000_000;

/// Writers made by this process so far, for their file names.
static INSTANCE: AtomicU32 = AtomicU32::new(0);

/// One hop's measurements. Peak and RMS are linear, 1.0 being full scale;
/// the spectrum is the bank per channel, a full-scale sine reading 1.0 in
/// its band, on `scale` from a `window` samples long FFT; a one-channel
/// bank fills both. The hold is the bank's peak hold in the same layout,
/// and the onsets are its four groups' bits.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Frame {
    pub seq: u64,
    pub time_ns: u64,
    pub frames: u64,
    pub peak: [f32; MAX_CHANNELS],
    pub rms: [f32; MAX_CHANNELS],
    pub spectrum: [Vec<f32>; MAX_CHANNELS],
    pub hold: [Vec<f32>; MAX_CHANNELS],
    pub onsets: u8,
    pub scale: Scale,
    pub window: u32,
    /// One-bit audio (DSD, DoP): the peak and RMS are density levels and
    /// the bank is empty.
    pub one_bit: bool,
}

/// The hops that arrived between two looks at the ring, as one: the peaks,
/// RMS and bands take the highest of them, the count and sequence the
/// latest, so a transient inside one frame of a display still shows.
pub fn merge_hops(hops: impl Iterator<Item = Frame>) -> Option<Frame> {
    let mut merged: Option<Frame> = None;
    for frame in hops {
        match merged.as_mut() {
            None => merged = Some(frame),
            Some(m) => {
                for ch in 0..MAX_CHANNELS {
                    m.peak[ch] = m.peak[ch].max(frame.peak[ch]);
                    m.rms[ch] = m.rms[ch].max(frame.rms[ch]);
                    for (a, b) in m.spectrum[ch].iter_mut().zip(frame.spectrum[ch].iter()) {
                        *a = a.max(*b);
                    }
                    for (a, b) in m.hold[ch].iter_mut().zip(frame.hold[ch].iter()) {
                        *a = a.max(*b);
                    }
                }
                m.onsets |= frame.onsets;
                m.one_bit |= frame.one_bit;
                m.scale = frame.scale;
                m.window = frame.window;
                m.frames = frame.frames;
                m.seq = frame.seq;
                m.time_ns = frame.time_ns;
            }
        }
    }
    merged
}

/// What the header says about the stream and the ring.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Info {
    pub rate: u32,
    pub channels: u32,
    pub fft_size: u32,
    pub bins: u32,
    pub slots: u32,
    pub slot_bytes: u32,
    pub pid: u32,
    pub hop: u32,
    pub seq: u64,
    pub written_ns: u64,
    /// The bank's scale, as `Scale::byte` gives it.
    pub scale: u32,
    pub version: u32,
}

const H_MAGIC: usize = 0;
const H_VERSION: usize = 8;
const H_HEADER_BYTES: usize = 12;
const H_RATE: usize = 16;
const H_CHANNELS: usize = 20;
const H_FFT: usize = 24;
const H_BINS: usize = 28;
const H_SLOTS: usize = 32;
const H_SLOT_BYTES: usize = 36;
const H_PID: usize = 40;
const H_HOP: usize = 44;
const H_SEQ: usize = 48;
const H_WRITTEN: usize = 56;
const H_SCALE: usize = 64;

const S_SEQ: usize = 0;
const S_TIME: usize = 8;
const S_FRAMES: usize = 16;
const S_PEAK: usize = 24;
const S_RMS: usize = 32;
const S_SPECTRUM: usize = 40;
/// After the tail sequence: the onset bits in the low nibble, bit 4 for one-bit audio.
const S_FLAGS_AFTER_TAIL: usize = 8;
const FLAG_ONE_BIT: u32 = 1 << 4;

/// The bytes of a slot: the head, the peaks and RMS, the bank per channel,
/// the hold per channel, the tail sequence and the flags, to a cache line.
fn slot_bytes(bins: usize) -> usize {
    let raw = S_SPECTRUM + bins * MAX_CHANNELS * 4 * 2 + 8 + 4;
    raw.div_ceil(64) * 64
}

/// Where a slot's tail sequence sits.
fn tail_at(bins: usize) -> usize {
    S_SPECTRUM + bins * MAX_CHANNELS * 4 * 2
}

/// Nanoseconds of the monotonic clock.
pub fn now_ns() -> u64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // A valid clock id and a valid pointer: this cannot fail.
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64
}

/// Remove the rings left under `dir` by writers whose process is gone: a
/// player killed outright never drops its writer. Every new writer sweeps
/// first. Returns how many went.
pub fn sweep(dir: &Path) -> usize {
    let mut gone = 0;
    let own = std::process::id() as i32;
    for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(rest) = name.strip_prefix(PREFIX) else {
            continue;
        };
        // `<tag>.<pid>.<n>`.
        let fields: Vec<&str> = rest.split('.').collect();
        let pid = match fields.len() {
            n if n >= 3 => fields[n - 2].parse::<i32>().ok(),
            _ => None,
        };
        let Some(pid) = pid else {
            continue;
        };
        if pid == own {
            continue;
        }
        // A process of another user answers with EPERM: alive.
        let alive = unsafe { libc::kill(pid, 0) } == 0
            || io::Error::last_os_error().raw_os_error() == Some(libc::EPERM);
        if !alive && fs::remove_file(entry.path()).is_ok() {
            gone += 1;
        }
    }
    gone
}

/// A mapped file. Unmapped when dropped.
struct Map {
    ptr: *mut u8,
    len: usize,
}

impl Map {
    fn open(fd: i32, len: usize, write: bool) -> io::Result<Self> {
        let prot = if write {
            libc::PROT_READ | libc::PROT_WRITE
        } else {
            libc::PROT_READ
        };
        // A shared mapping of `len` bytes of an open file.
        let ptr = unsafe { libc::mmap(std::ptr::null_mut(), len, prot, libc::MAP_SHARED, fd, 0) };
        if ptr == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            ptr: ptr as *mut u8,
            len,
        })
    }

    fn bytes(&self) -> &[u8] {
        // The mapping is `len` bytes long for as long as `self` lives.
        unsafe { std::slice::from_raw_parts(self.ptr, self.len) }
    }

    fn bytes_mut(&mut self) -> &mut [u8] {
        unsafe { std::slice::from_raw_parts_mut(self.ptr, self.len) }
    }

    fn atomic_u64(&self, at: usize) -> &AtomicU64 {
        // Every atomic offset is 8-aligned within a page-aligned mapping.
        unsafe { &*(self.ptr.add(at) as *const AtomicU64) }
    }
}

impl Drop for Map {
    fn drop(&mut self) {
        unsafe { libc::munmap(self.ptr as *mut libc::c_void, self.len) };
    }
}

// The ring is shared between processes by design; the mapping itself is
// owned by one writer or one reader at a time.
unsafe impl Send for Map {}

fn put_u32(bytes: &mut [u8], at: usize, v: u32) {
    bytes[at..at + 4].copy_from_slice(&v.to_le_bytes());
}
fn get_u32(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}
fn put_u64(bytes: &mut [u8], at: usize, v: u64) {
    bytes[at..at + 8].copy_from_slice(&v.to_le_bytes());
}
fn get_u64(bytes: &[u8], at: usize) -> u64 {
    let mut b = [0u8; 8];
    b.copy_from_slice(&bytes[at..at + 8]);
    u64::from_le_bytes(b)
}
fn put_f32(bytes: &mut [u8], at: usize, v: f32) {
    bytes[at..at + 4].copy_from_slice(&v.to_le_bytes());
}
fn get_f32(bytes: &[u8], at: usize) -> f32 {
    f32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

/// The writing side. Creating one creates the file; dropping it removes it.
pub struct Writer {
    map: Map,
    path: PathBuf,
    bins: usize,
    slots: usize,
    slot_bytes: usize,
    seq: u64,
}

impl Writer {
    /// Create the ring for a stream measured as `demand` asks. `slots` is
    /// how many hops are kept; the reader takes the latest, a history view
    /// can take more. The file is world-readable: the writer is the player's
    /// user, the reader is `volumio`.
    pub fn create(
        dir: &Path,
        tag: &str,
        rate: u32,
        channels: u32,
        demand: bank::Demand,
        hop: u32,
        slots: usize,
    ) -> io::Result<Self> {
        sweep(dir);
        let demand = demand.clean();
        let bins = demand.bins;
        let slots = slots.max(2);
        let slot_bytes = slot_bytes(bins);
        let len = HEADER_BYTES + slots * slot_bytes;
        // One file per writer: a process may hold more than one tap.
        let instance = INSTANCE.fetch_add(1, Ordering::Relaxed);
        let path = dir.join(format!("{PREFIX}{tag}.{}.{instance}", std::process::id()));
        let cpath = CString::new(path.to_string_lossy().as_bytes())
            .map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
        let fd = unsafe {
            libc::open(
                cpath.as_ptr(),
                libc::O_RDWR | libc::O_CREAT | libc::O_CLOEXEC,
                0o644,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let sized = unsafe { libc::ftruncate(fd, len as libc::off_t) };
        if sized != 0 {
            let err = io::Error::last_os_error();
            unsafe { libc::close(fd) };
            return Err(err);
        }
        let map = Map::open(fd, len, true);
        unsafe { libc::close(fd) };
        let mut map = map?;
        {
            let bytes = map.bytes_mut();
            bytes[..len].fill(0);
            bytes[H_MAGIC..H_MAGIC + 8].copy_from_slice(MAGIC);
            put_u32(bytes, H_VERSION, VERSION);
            put_u32(bytes, H_HEADER_BYTES, HEADER_BYTES as u32);
            put_u32(bytes, H_RATE, rate);
            put_u32(bytes, H_CHANNELS, channels.min(MAX_CHANNELS as u32));
            put_u32(bytes, H_FFT, demand.window as u32);
            put_u32(bytes, H_BINS, bins as u32);
            put_u32(bytes, H_SLOTS, slots as u32);
            put_u32(bytes, H_SLOT_BYTES, slot_bytes as u32);
            put_u32(bytes, H_PID, std::process::id());
            put_u32(bytes, H_HOP, hop);
            put_u32(bytes, H_SCALE, u32::from(demand.scale.byte()));
        }
        Ok(Self {
            map,
            path,
            bins,
            slots,
            slot_bytes,
            seq: 0,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Publish one hop. The frame's `seq` and `time_ns` are set here.
    pub fn publish(&mut self, frame: &Frame) {
        self.seq += 1;
        let seq = self.seq;
        let time = now_ns();
        let at = HEADER_BYTES + ((seq as usize) % self.slots) * self.slot_bytes;
        let bins = self.bins;
        {
            let bytes = self.map.bytes_mut();
            let slot = &mut bytes[at..at + self.slot_bytes];
            // The sequence at the head marks the slot as being written.
            put_u64(slot, S_SEQ, seq | (1 << 63));
            put_u64(slot, S_TIME, time);
            put_u64(slot, S_FRAMES, frame.frames);
            for ch in 0..MAX_CHANNELS {
                put_f32(slot, S_PEAK + ch * 4, frame.peak[ch]);
                put_f32(slot, S_RMS + ch * 4, frame.rms[ch]);
                let base = S_SPECTRUM + ch * bins * 4;
                for k in 0..bins {
                    put_f32(
                        slot,
                        base + k * 4,
                        frame.spectrum[ch].get(k).copied().unwrap_or(0.0),
                    );
                }
                let base = S_SPECTRUM + (MAX_CHANNELS + ch) * bins * 4;
                for k in 0..bins {
                    put_f32(
                        slot,
                        base + k * 4,
                        frame.hold[ch].get(k).copied().unwrap_or(0.0),
                    );
                }
            }
            put_u64(slot, tail_at(bins), seq);
            let one_bit = if frame.one_bit { FLAG_ONE_BIT } else { 0 };
            put_u32(
                slot,
                tail_at(bins) + S_FLAGS_AFTER_TAIL,
                u32::from(frame.onsets & 0x0f) | one_bit,
            );
            put_u64(slot, S_SEQ, seq);
        }
        self.map
            .atomic_u64(H_WRITTEN)
            .store(time, Ordering::Release);
        self.map.atomic_u64(H_SEQ).store(seq, Ordering::Release);
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// The reading side of one ring.
pub struct Reader {
    map: Map,
    path: PathBuf,
    info: Info,
}

impl Reader {
    /// Open a ring by path.
    pub fn open(path: &Path) -> io::Result<Self> {
        let meta = fs::metadata(path)?;
        let len = meta.len() as usize;
        if len < HEADER_BYTES {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "ring too short"));
        }
        let cpath = CString::new(path.to_string_lossy().as_bytes())
            .map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
        let fd = unsafe { libc::open(cpath.as_ptr(), libc::O_RDONLY | libc::O_CLOEXEC) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let map = Map::open(fd, len, false);
        unsafe { libc::close(fd) };
        let map = map?;
        let bytes = map.bytes();
        let version = get_u32(bytes, H_VERSION);
        if &bytes[H_MAGIC..H_MAGIC + 8] != MAGIC || version != VERSION {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "not a justoled ring",
            ));
        }
        let info = Info {
            rate: get_u32(bytes, H_RATE),
            channels: get_u32(bytes, H_CHANNELS),
            fft_size: get_u32(bytes, H_FFT),
            bins: get_u32(bytes, H_BINS),
            scale: get_u32(bytes, H_SCALE),
            slots: get_u32(bytes, H_SLOTS),
            slot_bytes: get_u32(bytes, H_SLOT_BYTES),
            pid: get_u32(bytes, H_PID),
            hop: get_u32(bytes, H_HOP),
            seq: 0,
            written_ns: 0,
            version,
        };
        let needed = HEADER_BYTES + info.slots as usize * info.slot_bytes as usize;
        if info.slots == 0
            || info.slot_bytes as usize != slot_bytes(info.bins as usize)
            || len < needed
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "ring header disagrees with its size",
            ));
        }
        Ok(Self {
            map,
            path: path.to_path_buf(),
            info,
        })
    }

    /// Every ring under `dir`, the most recently written first.
    pub fn find(dir: &Path) -> Vec<PathBuf> {
        let mut found: Vec<(u64, PathBuf)> = fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|entry| entry.file_name().to_string_lossy().starts_with(PREFIX))
            .filter_map(|entry| {
                let path = entry.path();
                let reader = Reader::open(&path).ok()?;
                Some((reader.info().written_ns, path))
            })
            .collect();
        found.sort_by_key(|a| std::cmp::Reverse(a.0));
        found.into_iter().map(|(_, p)| p).collect()
    }

    /// The ring being written right now, if any.
    pub fn open_live(dir: &Path) -> Option<Self> {
        Self::find(dir)
            .into_iter()
            .find_map(|path| Reader::open(&path).ok().filter(Reader::is_live))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The header, with the sequence and time of the last publish.
    pub fn info(&self) -> Info {
        Info {
            seq: self.map.atomic_u64(H_SEQ).load(Ordering::Acquire),
            written_ns: self.map.atomic_u64(H_WRITTEN).load(Ordering::Acquire),
            ..self.info
        }
    }

    /// Written within the last few seconds and still on disk.
    pub fn is_live(&self) -> bool {
        let written = self.map.atomic_u64(H_WRITTEN).load(Ordering::Acquire);
        written != 0 && now_ns().saturating_sub(written) < LIVE_NS && self.path.exists()
    }

    /// The last published hop, or `None` before the first, or when the
    /// writer was mid-write three times in a row.
    pub fn latest(&self) -> Option<Frame> {
        let seq = self.map.atomic_u64(H_SEQ).load(Ordering::Acquire);
        if seq == 0 {
            return None;
        }
        self.slot(seq)
    }

    /// The hop with a given sequence, while the ring still holds it.
    pub fn slot(&self, seq: u64) -> Option<Frame> {
        let latest = self.map.atomic_u64(H_SEQ).load(Ordering::Acquire);
        if seq == 0 || seq > latest || latest - seq >= self.info.slots as u64 {
            return None;
        }
        let bins = self.info.bins as usize;
        let tail_offset = tail_at(bins);
        let at = HEADER_BYTES
            + ((seq as usize) % self.info.slots as usize) * self.info.slot_bytes as usize;
        for _ in 0..3 {
            let bytes = self.map.bytes();
            let slot = &bytes[at..at + self.info.slot_bytes as usize];
            if get_u64(slot, S_SEQ) != seq || get_u64(slot, tail_offset) != seq {
                std::hint::spin_loop();
                continue;
            }
            let flags = get_u32(slot, tail_offset + S_FLAGS_AFTER_TAIL);
            let mut frame = Frame {
                seq,
                time_ns: get_u64(slot, S_TIME),
                frames: get_u64(slot, S_FRAMES),
                onsets: (flags & 0x0f) as u8,
                one_bit: flags & FLAG_ONE_BIT != 0,
                scale: Scale::from_byte(self.info.scale as u8).unwrap_or_default(),
                window: self.info.fft_size,
                ..Frame::default()
            };
            for ch in 0..MAX_CHANNELS {
                frame.peak[ch] = get_f32(slot, S_PEAK + ch * 4);
                frame.rms[ch] = get_f32(slot, S_RMS + ch * 4);
                let base = S_SPECTRUM + ch * bins * 4;
                frame.spectrum[ch] = (0..bins).map(|k| get_f32(slot, base + k * 4)).collect();
                let base = S_SPECTRUM + (MAX_CHANNELS + ch) * bins * 4;
                frame.hold[ch] = (0..bins).map(|k| get_f32(slot, base + k * 4)).collect();
            }
            // The head is written last; if it still matches, the copy is whole.
            if get_u64(slot, S_SEQ) == seq {
                return Some(frame);
            }
        }
        None
    }

    /// The hops published after `after`, oldest first, as many as the
    /// ring still holds; a reader that fell far behind gets the newest.
    pub fn since(&self, after: u64) -> Vec<Frame> {
        let latest = self.map.atomic_u64(H_SEQ).load(Ordering::Acquire);
        if latest == 0 || after >= latest {
            return Vec::new();
        }
        let oldest = latest.saturating_sub(self.info.slots as u64 - 1).max(1);
        let first = (after + 1).max(oldest);
        (first..=latest).filter_map(|seq| self.slot(seq)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("justoled-ring-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn rings_of_dead_processes_are_swept_and_live_ones_kept() {
        let dir = temp_dir("sweep");
        let dead = dir.join(format!("{PREFIX}test.{}.0", i32::MAX));
        let live = dir.join(format!("{PREFIX}test.{}.7", std::process::id()));
        let other = dir.join("not-a-ring.txt");
        for p in [&dead, &live, &other] {
            fs::write(p, b"x").unwrap();
        }
        assert_eq!(sweep(&dir), 1);
        assert!(!dead.exists());
        assert!(live.exists() && other.exists());
        fs::write(&dead, b"x").unwrap();
        let writer =
            Writer::create(&dir, "test", 48_000, 2, bank::Demand::default(), 128, 4).unwrap();
        assert!(!dead.exists(), "a new writer sweeps on its way in");
        assert!(writer.path().exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_frame_published_is_the_frame_read() {
        let dir = temp_dir("rw");
        let demand = bank::Demand::new(32, 2, Scale::Mel);
        let mut writer = Writer::create(&dir, "test", 48_000, 2, demand, 1024, 4).unwrap();
        let reader = Reader::open(writer.path()).unwrap();
        assert_eq!(reader.latest(), None, "nothing before the first publish");
        let frame = Frame {
            frames: 4096,
            peak: [0.5, 0.25],
            rms: [0.3, 0.2],
            spectrum: [
                (0..32).map(|k| k as f32 / 32.0).collect(),
                (0..32).map(|k| 1.0 - k as f32 / 32.0).collect(),
            ],
            hold: [
                (0..32).map(|k| k as f32 / 16.0).collect(),
                (0..32).map(|k| 1.0 - k as f32 / 64.0).collect(),
            ],
            onsets: 0b0011,
            scale: Scale::Mel,
            window: 4096,
            ..Frame::default()
        };
        writer.publish(&frame);
        let read = reader.latest().expect("a frame");
        assert_eq!(read.seq, 1);
        assert_eq!(read.frames, 4096);
        assert_eq!(read.peak, frame.peak);
        assert_eq!(read.rms, frame.rms);
        assert_eq!(read.spectrum, frame.spectrum);
        assert_eq!(read.hold, frame.hold);
        assert_eq!((read.scale, read.window), (Scale::Mel, 4096));
        assert_eq!(read.onsets, 0b0011);
        assert!(!read.one_bit);
        assert!(read.time_ns > 0);
        writer.publish(&Frame {
            one_bit: true,
            ..frame.clone()
        });
        let read = reader.latest().expect("a one-bit frame");
        assert!(read.one_bit);
        let info = reader.info();
        assert_eq!(
            (
                info.rate,
                info.channels,
                info.fft_size,
                info.bins,
                info.hop,
                info.slots,
                info.scale
            ),
            (48_000, 2, 4096, 32, 1024, 4, 1)
        );
        assert!(reader.is_live());
        for _ in 0..5 {
            writer.publish(&frame);
        }
        assert_eq!(reader.info().seq, 7);
        assert!(reader.slot(7).is_some());
        assert!(reader.slot(4).is_some());
        assert!(reader.slot(3).is_none(), "overwritten");
        // Since 5: hops 6 and 7. Since 0, far behind: the four the ring holds.
        let since: Vec<u64> = reader.since(5).iter().map(|f| f.seq).collect();
        assert_eq!(since, vec![6, 7]);
        let since: Vec<u64> = reader.since(0).iter().map(|f| f.seq).collect();
        assert_eq!(since, vec![4, 5, 6, 7]);
        assert!(reader.since(7).is_empty());
        assert_eq!(Reader::find(&dir).len(), 1);
        assert!(Reader::open_live(&dir).is_some());
        let path = writer.path().to_path_buf();
        drop(writer);
        assert!(!path.exists(), "the file goes with the writer");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_foreign_or_damaged_file_is_refused() {
        let dir = temp_dir("bad");
        let short = dir.join(format!("{PREFIX}x.1.0"));
        fs::write(&short, b"tiny").unwrap();
        assert!(Reader::open(&short).is_err());
        let mut bytes = vec![0u8; HEADER_BYTES + 64];
        bytes[..8].copy_from_slice(b"GLASSTAP");
        let foreign = dir.join(format!("{PREFIX}y.1.0"));
        fs::write(&foreign, &bytes).unwrap();
        let err = |p: &Path| {
            Reader::open(p)
                .err()
                .map(|e| e.to_string())
                .unwrap_or_default()
        };
        assert_eq!(err(&foreign), "not a justoled ring");
        bytes[..8].copy_from_slice(MAGIC);
        put_u32(&mut bytes, H_VERSION, VERSION);
        put_u32(&mut bytes, H_BINS, 32);
        put_u32(&mut bytes, H_SLOTS, 4);
        put_u32(&mut bytes, H_SLOT_BYTES, 64);
        let wrong = dir.join(format!("{PREFIX}z.1.0"));
        fs::write(&wrong, &bytes).unwrap();
        assert!(err(&wrong).contains("disagrees"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn merged_hops_keep_the_loudest_and_the_latest() {
        let a = Frame {
            seq: 1,
            peak: [0.2, 0.9],
            rms: [0.1, 0.5],
            spectrum: [vec![0.1, 0.9], vec![0.0; 2]],
            hold: [vec![0.5, 0.9], vec![0.0; 2]],
            onsets: 0b0001,
            ..Frame::default()
        };
        let b = Frame {
            seq: 2,
            peak: [0.8, 0.1],
            rms: [0.6, 0.1],
            spectrum: [vec![0.7, 0.2], vec![0.0; 2]],
            hold: [vec![0.7, 0.3], vec![0.0; 2]],
            onsets: 0b0100,
            one_bit: true,
            frames: 99,
            ..Frame::default()
        };
        let m = merge_hops(vec![a, b].into_iter()).unwrap();
        assert_eq!(m.seq, 2);
        assert_eq!(m.frames, 99);
        assert_eq!(m.peak, [0.8, 0.9]);
        assert_eq!(m.rms, [0.6, 0.5]);
        assert_eq!(m.spectrum[0], vec![0.7, 0.9]);
        assert_eq!(m.hold[0], vec![0.7, 0.9]);
        assert_eq!(m.onsets, 0b0101);
        assert!(m.one_bit);
        assert!(merge_hops(std::iter::empty()).is_none());
    }
}
