//! Signals and the system's intent. SIGTERM and SIGINT ask the daemon to
//! leave; SIGHUP asks it to read its settings again. When asked to leave,
//! the daemon looks at systemd's job queue to tell a shutdown or a reboot
//! from a plain stop, so the panel can say which.

use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use justoled::sys::run_with_timeout;

use crate::control::Farewell;

pub const TERM: u32 = 1;
pub const HUP: u32 = 2;

static PENDING: AtomicU32 = AtomicU32::new(0);

extern "C" fn on_signal(sig: libc::c_int) {
    let bit = match sig {
        libc::SIGTERM | libc::SIGINT => TERM,
        libc::SIGHUP => HUP,
        _ => return,
    };
    PENDING.fetch_or(bit, Ordering::SeqCst);
}

/// Catch the three signals; a store into a flag is all the handler does.
pub fn install() {
    for sig in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP] {
        unsafe {
            let mut action: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = on_signal as extern "C" fn(libc::c_int) as usize;
            libc::sigemptyset(&mut action.sa_mask);
            action.sa_flags = libc::SA_RESTART;
            libc::sigaction(sig, &action, std::ptr::null_mut());
        }
    }
    // A plugin that went away mid-write costs a failed write, not the daemon.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_IGN);
    }
}

/// The signals since the last call, as bits, cleared.
pub fn take() -> u32 {
    PENDING.swap(0, Ordering::SeqCst)
}

/// Whether the system is on its way down, and how: a reboot, halt or
/// power-off job in systemd's queue. `None` when the stop is the daemon's
/// alone, or when systemd cannot be asked in time.
pub fn pending_shutdown() -> Option<Farewell> {
    let jobs = run_with_timeout(
        &["systemctl", "list-jobs", "--no-legend", "--no-pager"],
        Duration::from_millis(1500),
    )?;
    classify_jobs(&jobs)
}

fn classify_jobs(jobs: &str) -> Option<Farewell> {
    let mut found = None;
    for line in jobs.lines() {
        if line.contains("reboot.target") || line.contains("kexec.target") {
            return Some(Farewell::Reboot);
        }
        if line.contains("poweroff.target")
            || line.contains("halt.target")
            || line.contains("shutdown.target")
        {
            found = Some(Farewell::Shutdown);
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn systemd_s_queue_is_read_for_the_way_down() {
        assert_eq!(classify_jobs(""), None);
        assert_eq!(classify_jobs("12 just-oled.service stop waiting\n"), None);
        assert_eq!(
            classify_jobs("1 poweroff.target start waiting\n2 umount.target stop running\n"),
            Some(Farewell::Shutdown)
        );
        assert_eq!(
            classify_jobs("1 shutdown.target start waiting\n3 reboot.target start waiting\n"),
            Some(Farewell::Reboot)
        );
    }

    #[test]
    fn a_raised_signal_is_taken_once() {
        install();
        assert_eq!(take(), 0);
        unsafe {
            libc::raise(libc::SIGHUP);
        }
        assert_eq!(take() & HUP, HUP);
        assert_eq!(take(), 0);
    }
}
