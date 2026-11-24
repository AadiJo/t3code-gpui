//! Hands freed heap pages back to the system after the client drops a large batch of rows.
//!
//! The system allocators keep freed small blocks for reuse, so closing the Settled shelf frees
//! its rows without lowering resident memory. [`release_after_drop`] asks the allocator to return
//! free pages (glibc `malloc_trim`, macOS `malloc_zone_pressure_relief`) a moment after the drop,
//! once the views that hold copies of the rows have rebuilt without them.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// How long the views get to drop their copies before the release.
const SETTLE: Duration = Duration::from_millis(750);

static PENDING: AtomicBool = AtomicBool::new(false);

/// Releases free heap pages shortly, once; calls while one is pending join it.
pub fn release_after_drop() {
    if PENDING.swap(true, Ordering::AcqRel) {
        return;
    }
    let spawned = std::thread::Builder::new().name("memory-release".into()).spawn(|| {
        std::thread::sleep(SETTLE);
        PENDING.store(false, Ordering::Release);
        release_free_pages();
    });
    if spawned.is_err() {
        PENDING.store(false, Ordering::Release);
    }
}

/// Returns the allocator's free pages to the system now.
pub fn release_free_pages() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    // SAFETY: malloc_trim only walks the allocator's own free lists.
    unsafe {
        libc::malloc_trim(0);
    }
    #[cfg(target_os = "macos")]
    {
        unsafe extern "C" {
            fn malloc_zone_pressure_relief(zone: *mut std::ffi::c_void, goal: usize) -> usize;
        }
        // SAFETY: a null zone means every zone; a goal of 0 releases all it can.
        unsafe {
            malloc_zone_pressure_relief(std::ptr::null_mut(), 0);
        }
    }
}
