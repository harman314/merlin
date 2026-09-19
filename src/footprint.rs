//! What the operating system charges this process.
//!
//! Every other figure in the memory report is something this app tracks. This
//! one is the number Activity Monitor shows, so the gap between them is the
//! memory nothing here accounts for.

/// Physical footprint in bytes, or `None` where it cannot be read.
pub fn bytes() -> Option<u64> {
    imp::bytes()
}

#[cfg(target_os = "macos")]
mod imp {
    pub(super) fn bytes() -> Option<u64> {
        let mut info = std::mem::MaybeUninit::<libc::rusage_info_v2>::zeroed();
        // SAFETY: the call fills a buffer of the flavour it is told, and
        // rusage_info_v2 is that flavour's layout.
        let ok = unsafe {
            libc::proc_pid_rusage(
                std::process::id() as libc::c_int,
                libc::RUSAGE_INFO_V2,
                info.as_mut_ptr().cast(),
            )
        };
        (ok == 0).then(|| unsafe { info.assume_init() }.ri_phys_footprint)
    }
}

#[cfg(target_os = "linux")]
mod imp {
    pub(super) fn bytes() -> Option<u64> {
        // Second field of statm is resident pages.
        let statm = std::fs::read_to_string("/proc/self/statm").ok()?;
        let pages: u64 = statm.split_whitespace().nth(1)?.parse().ok()?;
        Some(pages * 4096)
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
mod imp {
    pub(super) fn bytes() -> Option<u64> {
        None
    }
}
