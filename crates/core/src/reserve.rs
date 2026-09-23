//! Memory set aside for the moment the machine has none.
//!
//! A Rust program whose allocation is refused ends there and then: the
//! standard library aborts, and there is no error to handle. On a machine
//! whose commit a build or a runaway python has used up, the refused program
//! is whichever asks next -- and a terminal that keeps AIs at work asks all
//! the time, a few bytes at once. It went three times in two days that way,
//! each time over an allocation smaller than a page.
//!
//! So some memory is committed at the start and never touched, and the
//! allocator this program uses ([`Reserve`]) gives it back to Windows the
//! instant an allocation is refused, then asks again. A shortage that lasts
//! the seconds a build takes to link is ridden out. Committed and untouched,
//! it takes none of the machine's RAM, only a place in its commit.
//!
//! Nothing here allocates or logs: this runs *inside* a refused allocation.
//! What happened is picked up afterwards by `crate::pressure`, which says so
//! and puts the reserve back once there is room.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// How much is set aside. Many times what this program allocates in a
/// second, and small enough for the smallest machine it runs on not to
/// notice the commit it takes
pub const SIZE: usize = 64 * 1024 * 1024;

/// The reserve's address, or 0 while there is none
static HELD: AtomicUsize = AtomicUsize::new(0);

/// Set when the reserve was given back to keep this program running, and
/// cleared by whoever reports it
static SPENT: AtomicBool = AtomicBool::new(false);

/// Set the reserve aside. True if it is now held (whether by this call or
/// an earlier one)
pub fn arm() -> bool {
    if armed() {
        return true;
    }
    let Some(at) = commit(SIZE) else { return false };
    if HELD.compare_exchange(0, at, Ordering::SeqCst, Ordering::SeqCst).is_err() {
        // Another thread set one aside first; one is enough
        unsafe { release(at) };
    }
    true
}

/// Whether the reserve is held right now
pub fn armed() -> bool {
    HELD.load(Ordering::SeqCst) != 0
}

/// Whether the reserve has been spent since this was last asked. Asking
/// clears it, so each rescue is reported once
pub fn take_spent() -> bool {
    SPENT.swap(false, Ordering::SeqCst)
}

/// Give the reserve back to Windows, if it is held. True if it was: there is
/// now that much more room, and an allocation that was refused is worth
/// asking for again
fn spend() -> bool {
    let at = HELD.swap(0, Ordering::SeqCst);
    if at == 0 {
        return false;
    }
    unsafe { release(at) };
    SPENT.store(true, Ordering::SeqCst);
    true
}

#[cfg(windows)]
fn commit(size: usize) -> Option<usize> {
    use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEM_RESERVE, PAGE_READWRITE, VirtualAlloc};
    let at = unsafe { VirtualAlloc(std::ptr::null(), size, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE) };
    (!at.is_null()).then_some(at as usize)
}

#[cfg(windows)]
unsafe fn release(at: usize) {
    use windows_sys::Win32::System::Memory::{MEM_RELEASE, VirtualFree};
    unsafe { VirtualFree(at as *mut _, 0, MEM_RELEASE) };
}

// Elsewhere the kernel overcommits: an allocation is not refused but a
// program is ended later, of the kernel's choosing, so there is nothing to
// set aside against
#[cfg(not(windows))]
fn commit(_size: usize) -> Option<usize> {
    None
}

#[cfg(not(windows))]
unsafe fn release(_at: usize) {}

/// The allocator: the system's own, with one more try when it says no.
///
/// Put in place by the program, not by this library:
///
/// ```ignore
/// #[global_allocator]
/// static ALLOC: shikisha_core::reserve::Reserve = shikisha_core::reserve::Reserve;
/// ```
pub struct Reserve;

unsafe impl GlobalAlloc for Reserve {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(layout) };
        if p.is_null() && spend() {
            return unsafe { System.alloc(layout) };
        }
        p
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { System.alloc_zeroed(layout) };
        if p.is_null() && spend() {
            return unsafe { System.alloc_zeroed(layout) };
        }
        p
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let p = unsafe { System.realloc(ptr, layout, new_size) };
        if p.is_null() && spend() {
            return unsafe { System.realloc(ptr, layout, new_size) };
        }
        p
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    /// A refused allocation spends the reserve, is asked for again, and the
    /// rescue is reported once. The refusal is made real by asking for more
    /// than any machine has, so the second ask is refused too -- which is also
    /// the case to get right: nothing spent twice, nothing invented
    #[test]
    fn a_refusal_spends_the_reserve_once_and_says_so() {
        assert!(arm(), "no reserve could be set aside");
        assert!(armed());
        let impossible = Layout::from_size_align(1 << 46, 16).unwrap();
        let p = unsafe { Reserve.alloc(impossible) };
        assert!(p.is_null(), "64 TB was granted");
        assert!(!armed(), "the reserve was given back");
        assert!(take_spent(), "and that was recorded");
        assert!(!take_spent(), "once");
        let p = unsafe { Reserve.alloc(impossible) };
        assert!(p.is_null());
        assert!(!take_spent(), "with nothing held, nothing is spent");
        assert!(arm(), "and it can be set aside again");
        // An ordinary allocation goes straight through
        let small = Layout::from_size_align(64, 8).unwrap();
        let p = unsafe { Reserve.alloc(small) };
        assert!(!p.is_null());
        unsafe { Reserve.dealloc(p, small) };
        assert!(armed(), "an allocation that succeeds leaves the reserve alone");
    }
}
