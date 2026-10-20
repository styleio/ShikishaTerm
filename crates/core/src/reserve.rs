//! Memory set aside for the moment the machine has none.
//!
//! A Rust program whose allocation is refused ends there and then: the
//! standard library aborts, and there is no error to handle. On a machine
//! whose commit a build or a runaway python has used up, the refused program
//! is whichever asks next -- and a terminal that keeps AIs at work asks all
//! the time, a few bytes at once. It went three times in two days that way,
//! each time over an allocation smaller than a page.
//!
//! So some memory is committed at the start and kept: when the system refuses
//! an allocation, the allocator this program uses ([`Reserve`]) hands it out
//! of the reserve instead, and takes it back there when it is freed. A
//! shortage that lasts the seconds a build takes to link is ridden out.
//! Committed and untouched until then, it takes none of the machine's RAM,
//! only a place in its commit.
//!
//! The reserve is never given back to Windows. It once was, on the refusal,
//! so the allocation could be asked for again -- but memory given back is the
//! machine's, not this program's, and on a machine being run dry the program
//! running it dry takes it first. On 2026-10-19 the reserve went back at a
//! refusal while a link was growing by the gigabyte, and the run ended a
//! second later. A probe that limits only itself (`bin/reserve_probe.rs`)
//! cannot show that: nothing else there is asking.
//!
//! Nothing here allocates or logs: this runs *inside* a refused allocation.
//! What happened is picked up afterwards by `crate::pressure`, which says so.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// How much is set aside. Many times what this program holds onto through
/// a shortage of a few seconds, and small enough for the smallest machine it
/// runs on not to notice the commit it takes
pub const SIZE: usize = 64 * 1024 * 1024;

/// The reserve's address, or 0 while there is none. Set once and kept
static HELD: AtomicUsize = AtomicUsize::new(0);

/// What hands out the reserve. Behind the standard lock, which neither
/// allocates nor gives way to another allocation while it is held
#[cfg(windows)]
static HEAP: std::sync::Mutex<linked_list_allocator::Heap> =
    std::sync::Mutex::new(linked_list_allocator::Heap::empty());

/// How much of the reserve is handed out right now, in bytes
static IN_USE: AtomicUsize = AtomicUsize::new(0);

/// Set whenever the reserve answers an allocation, and cleared by whoever
/// reports it. Set by each one, not only by the first after the reserve was
/// empty: a block lent in one shortage can outlive it, and the next shortage
/// would then never be said. Telling one shortage from the next is the
/// reporter's to do (`crate::pressure`)
static SPENT: AtomicBool = AtomicBool::new(false);

/// Set the reserve aside. True if it is now held (whether by this call or
/// an earlier one)
#[cfg(windows)]
pub fn arm() -> bool {
    if armed() {
        return true;
    }
    let mut heap = HEAP.lock().unwrap_or_else(|e| e.into_inner());
    // Another thread set one aside while this one waited; one is enough
    if armed() {
        return true;
    }
    let Some(at) = commit(SIZE) else { return false };
    // SAFETY: the range was just committed for this alone, is never given
    // back, and nothing else is told of it
    unsafe { heap.init(at as *mut u8, SIZE) };
    HELD.store(at, Ordering::SeqCst);
    true
}

// Elsewhere the kernel overcommits: an allocation is not refused but a
// program is ended later, of the kernel's choosing, so there is nothing to
// set aside against
#[cfg(not(windows))]
pub fn arm() -> bool {
    false
}

/// Whether the reserve is held
pub fn armed() -> bool {
    HELD.load(Ordering::SeqCst) != 0
}

/// How much of the reserve is handed out right now, in bytes
pub fn in_use() -> usize {
    IN_USE.load(Ordering::SeqCst)
}

/// Whether the reserve has been drawn on since this was last asked. Asking
/// clears it, so each rescue is reported once
pub fn take_spent() -> bool {
    SPENT.swap(false, Ordering::SeqCst)
}

/// Whether `p` was handed out of the reserve
fn ours(p: *mut u8) -> bool {
    let at = HELD.load(Ordering::SeqCst);
    at != 0 && (at..at + SIZE).contains(&(p as usize))
}

/// Hand out `layout` from the reserve, or null when it is not held or has no
/// room for it
#[cfg(windows)]
fn from_reserve(layout: Layout) -> *mut u8 {
    if !armed() {
        return std::ptr::null_mut();
    }
    let got = HEAP.lock().unwrap_or_else(|e| e.into_inner()).allocate_first_fit(layout);
    match got {
        Ok(p) => {
            IN_USE.fetch_add(layout.size(), Ordering::SeqCst);
            SPENT.store(true, Ordering::SeqCst);
            p.as_ptr()
        }
        Err(()) => std::ptr::null_mut(),
    }
}

#[cfg(not(windows))]
fn from_reserve(_layout: Layout) -> *mut u8 {
    std::ptr::null_mut()
}

/// Take back what [`from_reserve`] handed out
#[cfg(windows)]
unsafe fn to_reserve(p: *mut u8, layout: Layout) {
    // SAFETY: `p` came from the reserve with this layout, the caller says
    unsafe {
        HEAP.lock()
            .unwrap_or_else(|e| e.into_inner())
            .deallocate(std::ptr::NonNull::new_unchecked(p), layout)
    };
    IN_USE.fetch_sub(layout.size(), Ordering::SeqCst);
}

#[cfg(not(windows))]
unsafe fn to_reserve(_p: *mut u8, _layout: Layout) {}

#[cfg(windows)]
fn commit(size: usize) -> Option<usize> {
    use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEM_RESERVE, PAGE_READWRITE, VirtualAlloc};
    let at = unsafe { VirtualAlloc(std::ptr::null(), size, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE) };
    (!at.is_null()).then_some(at as usize)
}

/// What the system is asked first. A test stands in a system that refuses,
/// which a test cannot otherwise arrange without running the machine dry
#[cfg(not(test))]
fn refusing() -> bool {
    false
}

#[cfg(test)]
static REFUSING: AtomicBool = AtomicBool::new(false);

#[cfg(test)]
fn refusing() -> bool {
    REFUSING.load(Ordering::SeqCst)
}

/// The allocator: the system's own, with the reserve behind it when it says no.
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
        let p = if refusing() { std::ptr::null_mut() } else { unsafe { System.alloc(layout) } };
        if p.is_null() {
            return from_reserve(layout);
        }
        p
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let p = if refusing() { std::ptr::null_mut() } else { unsafe { System.alloc_zeroed(layout) } };
        if p.is_null() {
            // The reserve's memory is handed out again after it is freed, so
            // it is cleared here rather than trusted to be
            let q = from_reserve(layout);
            if !q.is_null() {
                unsafe { std::ptr::write_bytes(q, 0, layout.size()) };
            }
            return q;
        }
        p
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: the caller promises `new_size` rounded up to the alignment
        // does not overflow, which is what this layout needs
        let grown = unsafe { Layout::from_size_align_unchecked(new_size, layout.align()) };
        if !ours(ptr) {
            let p = if refusing() { std::ptr::null_mut() } else { unsafe { System.realloc(ptr, layout, new_size) } };
            if !p.is_null() {
                return p;
            }
            // Refused, the system leaves the old block where it was
            let q = from_reserve(grown);
            if !q.is_null() {
                unsafe {
                    std::ptr::copy_nonoverlapping(ptr, q, layout.size().min(new_size));
                    System.dealloc(ptr, layout);
                }
            }
            return q;
        }
        // A block of the reserve's moves out of it as soon as the system will
        // have it again, so the reserve is there for the next shortage
        let q = unsafe { self.alloc(grown) };
        if !q.is_null() {
            unsafe {
                std::ptr::copy_nonoverlapping(ptr, q, layout.size().min(new_size));
                to_reserve(ptr, layout);
            }
        }
        q
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if ours(ptr) {
            unsafe { to_reserve(ptr, layout) }
        } else {
            unsafe { System.dealloc(ptr, layout) }
        }
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    /// The tests share the one reserve and the one stand-in refusal, so
    /// they take turns
    static TURN: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// With the system refusing, allocations come out of the reserve, are
    /// taken back into it when freed, and the rescue is reported once however
    /// many there were. Asked for more than the reserve has, the answer is
    /// still no: nothing is invented
    #[test]
    fn a_refusal_is_answered_from_the_reserve_and_it_is_said_once() {
        let _turn = TURN.lock().unwrap_or_else(|e| e.into_inner());
        assert!(arm(), "no reserve could be set aside");
        assert!(armed());
        let _ = take_spent();
        let small = Layout::from_size_align(64, 8).unwrap();
        // An ordinary allocation goes straight through
        let p = unsafe { Reserve.alloc(small) };
        assert!(!p.is_null() && !ours(p), "the system's own allocation came from the reserve");
        unsafe { Reserve.dealloc(p, small) };
        assert!(!take_spent(), "an allocation that succeeds leaves the reserve alone");

        REFUSING.store(true, Ordering::SeqCst);
        let held: Vec<*mut u8> = (0..1000).map(|_| unsafe { Reserve.alloc(small) }).collect();
        let zeroed = unsafe { Reserve.alloc_zeroed(Layout::from_size_align(4096, 64).unwrap()) };
        let impossible = unsafe { Reserve.alloc(Layout::from_size_align(SIZE * 2, 16).unwrap()) };
        REFUSING.store(false, Ordering::SeqCst);

        assert!(held.iter().all(|p| !p.is_null() && ours(*p)), "a refusal was not answered from the reserve");
        assert!(!zeroed.is_null() && ours(zeroed) && (zeroed as usize).is_multiple_of(64), "the alignment asked for was not kept");
        assert!(unsafe { std::slice::from_raw_parts(zeroed, 4096) }.iter().all(|b| *b == 0));
        assert!(impossible.is_null(), "more than the reserve holds was handed out");
        assert!(in_use() >= 1000 * 64 + 4096);
        assert!(take_spent(), "the rescue was not recorded");
        assert!(!take_spent(), "a rescue was said twice");

        // Freed, it goes back to the reserve and nowhere else
        for p in held {
            unsafe { Reserve.dealloc(p, small) };
        }
        unsafe { Reserve.dealloc(zeroed, Layout::from_size_align(4096, 64).unwrap()) };
        assert_eq!(in_use(), 0, "the reserve did not get everything back");
        assert!(armed(), "the reserve was let go of");
    }

    /// A block grown while the system refuses moves into the reserve with its
    /// contents, and moves back out with them once the system answers again
    #[test]
    fn a_block_grown_through_a_shortage_keeps_what_it_holds() {
        let _turn = TURN.lock().unwrap_or_else(|e| e.into_inner());
        assert!(arm());
        let small = Layout::from_size_align(16, 8).unwrap();
        let p = unsafe { Reserve.alloc(small) };
        unsafe { std::ptr::copy_nonoverlapping(b"held through it.".as_ptr(), p, 16) };

        REFUSING.store(true, Ordering::SeqCst);
        let q = unsafe { Reserve.realloc(p, small, 4096) };
        REFUSING.store(false, Ordering::SeqCst);
        assert!(ours(q), "the grown block did not come from the reserve");
        assert_eq!(unsafe { std::slice::from_raw_parts(q, 16) }, b"held through it.");

        let grown = Layout::from_size_align(4096, 8).unwrap();
        let r = unsafe { Reserve.realloc(q, grown, 8192) };
        assert!(!ours(r), "the block stayed in the reserve after the shortage");
        assert_eq!(unsafe { std::slice::from_raw_parts(r, 16) }, b"held through it.");
        unsafe { Reserve.dealloc(r, Layout::from_size_align(8192, 8).unwrap()) };
    }
}
