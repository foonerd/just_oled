//! `libjustoledtap.so`, ALSA `type justoledtap`: heads the player's chain,
//! passes every stream through byte for byte, and measures it on a thread
//! of its own into a ring under `/dev/shm`. The ALSA entry points arrive
//! with the tap itself; this build carries the version so the shipped
//! object can be told apart.

use std::ffi::c_char;

/// The version of the tap, as a C string, for `scripts/run.sh --probe`.
#[no_mangle]
pub extern "C" fn justoledtap_version() -> *const c_char {
    concat!(env!("CARGO_PKG_VERSION"), "\0").as_ptr() as *const c_char
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CStr;

    #[test]
    fn the_version_is_the_workspace_version() {
        let v = unsafe { CStr::from_ptr(justoledtap_version()) };
        assert_eq!(v.to_str().unwrap(), justoled::VERSION);
    }
}
