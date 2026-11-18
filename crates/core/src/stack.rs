//! Where a thread of this program is: the functions it is inside of, from
//! the innermost out.
//!
//! "The work part has not answered for forty seconds" says that something
//! is wrong and not what. What says it is where the thread stands: inside
//! the system's call that reads a folder, or waits for a program to end, or
//! for an answer from the network -- and, above that, which of this
//! program's own functions asked. That is read here, from another thread,
//! the moment the watchdog finds a part stopped (`crate::watchdog`), and
//! written down as names and numbers. None of the thread's memory is kept:
//! not what it was working on, only where.
//!
//! A function of the system's is told by its name, which the system's own
//! files carry. One of this program's is told by its place in the program's
//! file -- the program is shipped without the names of its functions -- and
//! that place is turned into a name by whoever has the build it came from.

/// The most functions told, from the innermost out
#[cfg_attr(not(all(windows, target_arch = "x86_64")), allow(dead_code))]
const MOST: usize = 64;

/// The number the system knows the calling thread by; 0 where it has none
/// this module can use
pub fn this_thread() -> u32 {
    #[cfg(windows)]
    // SAFETY: a plain question of the system
    unsafe {
        windows_sys::Win32::System::Threading::GetCurrentThreadId()
    }
    #[cfg(not(windows))]
    0
}

/// Where the thread `thread` is, as text, a function a line. Empty where it
/// cannot be read: another kind of machine, a thread that is gone, or the
/// reading given up on
pub fn of(thread: u32) -> Vec<String> {
    places(thread).into_iter().map(told).collect()
}

/// The same as the places themselves, for somebody who asks often and
/// names each place once
pub fn places(thread: u32) -> Vec<u64> {
    #[cfg(all(windows, target_arch = "x86_64"))]
    {
        native::places(thread)
    }
    #[cfg(not(all(windows, target_arch = "x86_64")))]
    {
        let _ = thread;
        Vec::new()
    }
}

/// A place in the program, as text
pub fn told(place: u64) -> String {
    #[cfg(all(windows, target_arch = "x86_64"))]
    {
        native::told(place)
    }
    #[cfg(not(all(windows, target_arch = "x86_64")))]
    {
        format!("{place:#x}")
    }
}

#[cfg(all(windows, target_arch = "x86_64"))]
mod native {
    use super::MOST;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU8, Ordering};
    use windows_sys::Win32::Foundation::{CloseHandle, FreeLibrary, HANDLE, HMODULE};
    use windows_sys::Win32::System::Diagnostics::Debug::{CONTEXT, GetThreadContext, ReadProcessMemory, RtlLookupFunctionEntry, RtlVirtualUnwind, UNW_FLAG_NHANDLER};
    use windows_sys::Win32::System::LibraryLoader::{GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GetModuleFileNameW, GetModuleHandleExW};
    use windows_sys::Win32::System::Memory::{MEMORY_BASIC_INFORMATION, VirtualQuery};
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenThread, ResumeThread, SuspendThread, THREAD_GET_CONTEXT, THREAD_QUERY_INFORMATION, THREAD_SUSPEND_RESUME};

    /// The registers that say where a thread is and how to step out of it
    const WHERE: u32 = 0x0010_0003;
    /// How long the reading may take before the thread is let go on without it
    const GIVEN: std::time::Duration = std::time::Duration::from_millis(1500);

    // Where the reading stands. The thread is held still exactly once, by
    // the reader, and let go exactly once: by the reader when it is done, or
    // by whoever asked when the reader is held up -- and which of the two is
    // settled by who moves this on from HELD. Nobody lets go of a thread
    // that is not held, and nobody holds one after the asker has given up
    const NOT_BEGUN: u8 = 0;
    const HOLDING: u8 = 1;
    const HELD: u8 = 2;
    const LETTING_GO: u8 = 3;
    const DONE: u8 = 4;
    const GIVEN_UP: u8 = 5;

    /// The thread is held still while it is read, and a thread held still
    /// keeps whatever it holds -- the lock on the memory everybody
    /// allocates from, for one. So nothing is allocated while it is held:
    /// what is needed is made before. And the reading is done by a thread
    /// of its own, which whoever asked waits for only so long: if the held
    /// thread has the one lock the reading itself needs, the reader would
    /// wait for it for ever, with the thread it is waiting for held still
    /// by itself. Then whoever asked lets the thread go on, and does without
    pub fn places(thread: u32) -> Vec<u64> {
        if thread == 0 || thread == super::this_thread() {
            return Vec::new();
        }
        // SAFETY: a thread of this process, opened to be held and read; the handle is closed below
        let handle = unsafe { OpenThread(THREAD_SUSPEND_RESUME | THREAD_GET_CONTEXT | THREAD_QUERY_INFORMATION, 0, thread) };
        if handle.is_null() {
            return Vec::new();
        }
        let handle = handle as usize;
        let state = Arc::new(AtomicU8::new(NOT_BEGUN));
        let (tx, rx) = std::sync::mpsc::channel::<Vec<u64>>();
        let reader = {
            let state = state.clone();
            std::thread::Builder::new().name("shikisha-stack".into()).spawn(move || {
                let mut found = Vec::with_capacity(MOST);
                // SAFETY: zeroes are a context to be filled in, and the box keeps it lined up as the system wants
                let mut context: Box<CONTEXT> = Box::new(unsafe { std::mem::zeroed() });
                context.ContextFlags = WHERE;
                let handle = handle as HANDLE;
                // Begun too late, after whoever asked stopped waiting: the thread is not touched
                if state.compare_exchange(NOT_BEGUN, HOLDING, Ordering::SeqCst, Ordering::SeqCst).is_err() {
                    return;
                }
                // SAFETY: the handle is open for as long as the state says the reader may use it;
                // from the hold to the letting go nothing is allocated
                unsafe {
                    if SuspendThread(handle) == u32::MAX {
                        state.store(DONE, Ordering::SeqCst);
                        let _ = tx.send(found);
                        return;
                    }
                    state.store(HELD, Ordering::SeqCst);
                    let held = || state.load(Ordering::SeqCst) == HELD;
                    if GetThreadContext(handle, &mut *context) != 0 {
                        // Its stack, from where it stands to the top: a step
                        // that leads outside it is not taken
                        let mut region: MEMORY_BASIC_INFORMATION = std::mem::zeroed();
                        let top = match VirtualQuery(context.Rsp as *const _, &mut region, std::mem::size_of::<MEMORY_BASIC_INFORMATION>()) {
                            0 => 0,
                            _ => region.BaseAddress as u64 + region.RegionSize as u64,
                        };
                        while found.len() < MOST && held() {
                            let (at, stack) = (context.Rip, context.Rsp);
                            if at == 0 || stack < 8 || stack + 8 > top {
                                break;
                            }
                            found.push(at);
                            let mut base = 0u64;
                            let entry = RtlLookupFunctionEntry(at, &mut base, std::ptr::null_mut());
                            // The looking up is where a reader can be held up: whoever asked
                            // may have let the thread go on meanwhile, and what was read of it is stale
                            if !held() {
                                break;
                            }
                            if entry.is_null() {
                                // A function that keeps nothing on the stack: where it goes back to is on top
                                let (mut back, mut got) = (0u64, 0usize);
                                if ReadProcessMemory(GetCurrentProcess(), stack as *const _, &mut back as *mut u64 as *mut _, 8, &mut got) == 0 || got != 8 {
                                    break;
                                }
                                context.Rip = back;
                                context.Rsp = stack + 8;
                            } else {
                                let (mut data, mut frame) = (std::ptr::null_mut(), 0u64);
                                RtlVirtualUnwind(UNW_FLAG_NHANDLER, base, at, entry, &mut *context, &mut data, &mut frame, std::ptr::null_mut());
                            }
                            // Out of a function, the stack is further up, never back down
                            if context.Rsp <= stack {
                                break;
                            }
                        }
                    }
                    match state.compare_exchange(HELD, LETTING_GO, Ordering::SeqCst, Ordering::SeqCst) {
                        Ok(_) => {
                            ResumeThread(handle);
                            state.store(DONE, Ordering::SeqCst);
                        }
                        // Whoever asked let it go on, part of the way through: nothing of what was read is told
                        Err(_) => return,
                    }
                }
                let _ = tx.send(found);
            })
        };
        // SAFETY (for every close below): the handle was opened above, and is closed only once the
        // state says the reader will not use it again
        let close = || unsafe { CloseHandle(handle as HANDLE) };
        if reader.is_err() {
            close();
            return Vec::new();
        }
        if let Ok(found) = rx.recv_timeout(GIVEN) {
            close();
            return found;
        }
        loop {
            match state.load(Ordering::SeqCst) {
                // Not begun: it never will be
                NOT_BEGUN => {
                    if state.compare_exchange(NOT_BEGUN, GIVEN_UP, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
                        close();
                        return Vec::new();
                    }
                }
                // Held, and the reader held up: the thread goes on, and the reader stops at its next step
                HELD => {
                    if state.compare_exchange(HELD, GIVEN_UP, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
                        // SAFETY: held by the reader, once, and let go here, once
                        unsafe { ResumeThread(handle as HANDLE) };
                        close();
                        return Vec::new();
                    }
                }
                // Done in the moment the waiting ended: what it read is on its way
                DONE => {
                    let found = rx.recv_timeout(std::time::Duration::from_millis(200)).unwrap_or_default();
                    close();
                    return found;
                }
                // Being held, or being let go, this instant
                _ => std::thread::sleep(std::time::Duration::from_millis(1)),
            }
        }
    }

    /// A value out of a file of the program as it is laid out in memory
    ///
    /// SAFETY: `base + at` has to be inside a file the system has laid out and is keeping there
    unsafe fn read<T: Copy>(base: u64, at: u64) -> T {
        unsafe { std::ptr::read_unaligned((base + at) as *const T) }
    }

    /// The function of a system's file that `place` is in, by the names the
    /// file itself gives out, and how far into it
    fn named(base: u64, place: u64) -> Option<(String, u64)> {
        // SAFETY: `base` is where the system laid a file out, kept there by whoever calls;
        // every place read is checked against the file's own size
        unsafe {
            if read::<u16>(base, 0) != 0x5a4d {
                return None;
            }
            let head = read::<u32>(base, 0x3c) as u64;
            if head > 0x1000 {
                return None;
            }
            // Only the kind of file a 64-bit program is made of
            if read::<u32>(base, head) != 0x4550 || read::<u16>(base, head + 24) != 0x20b {
                return None;
            }
            let size = read::<u32>(base, head + 24 + 56) as u64;
            let (names_at, names_len) = (read::<u32>(base, head + 24 + 112) as u64, read::<u32>(base, head + 24 + 116) as u64);
            if names_at == 0 || names_at + 40 > size {
                return None;
            }
            let count = read::<u32>(base, names_at + 0x18) as u64;
            let (functions, names, numbers) = (read::<u32>(base, names_at + 0x1c) as u64, read::<u32>(base, names_at + 0x20) as u64, read::<u32>(base, names_at + 0x24) as u64);
            let all = read::<u32>(base, names_at + 0x14) as u64;
            if count > 100_000 || all > 200_000 || names + count * 4 > size || numbers + count * 2 > size || functions + all * 4 > size {
                return None;
            }
            let mut best: Option<(u64, u64)> = None;
            for i in 0..count {
                let number = read::<u16>(base, numbers + i * 2) as u64;
                if number >= all {
                    continue;
                }
                let starts = read::<u32>(base, functions + number * 4) as u64;
                // One that only points at another file's is not a function here
                let pointer = starts >= names_at && starts < names_at + names_len;
                if !pointer && starts <= place && best.is_none_or(|(b, _)| starts > b) {
                    best = Some((starts, read::<u32>(base, names + i * 4) as u64));
                }
            }
            let (starts, name) = best?;
            // Far into a function the file gives no name for
            if place - starts > 0x4000 || name >= size {
                return None;
            }
            let mut said = String::new();
            for k in 0..200u64 {
                if name + k >= size {
                    break;
                }
                match read::<u8>(base, name + k) {
                    0 => break,
                    c => said.push(c as char),
                }
            }
            Some((said, place - starts))
        }
    }

    pub fn told(place: u64) -> String {
        let mut module: HMODULE = std::ptr::null_mut();
        // The file is kept where it is until it has been read: asked for
        // this way, the system counts one more user of it, given back below
        // SAFETY: asked by a place inside it
        let found = unsafe { GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, place as *const u16, &mut module) };
        if found == 0 || module.is_null() {
            return format!("{place:#x}");
        }
        let mut name = [0u16; 512];
        // SAFETY: the room given is the room there is
        let len = unsafe { GetModuleFileNameW(module, name.as_mut_ptr(), name.len() as u32) } as usize;
        let whole = String::from_utf16_lossy(&name[..len.min(name.len())]);
        // The file's own name: where it is kept says who the person is
        let file = whole.rsplit(['\\', '/']).next().unwrap_or(&whole).to_string();
        let base = module as u64;
        let at = place.saturating_sub(base);
        let said = match named(base, at) {
            Some((function, into)) => format!("{file}!{function}+{into:#x}"),
            None => format!("{file}+{at:#x}"),
        };
        // SAFETY: the one use counted above
        unsafe { FreeLibrary(module) };
        said
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A thread that is waiting is found waiting: inside the system, with
    /// this program's own functions above it. And the thread goes on after
    #[test]
    fn a_waiting_thread_is_found_where_it_waits() {
        let (tx, rx) = std::sync::mpsc::channel::<u32>();
        let (go, wait) = std::sync::mpsc::channel::<()>();
        let waiting = std::thread::spawn(move || {
            let _ = tx.send(this_thread());
            // Held up here until told to go on
            let _ = wait.recv();
            7
        });
        let thread = rx.recv().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(200));
        let found = of(thread);
        let _ = go.send(());
        assert_eq!(waiting.join().unwrap(), 7, "the thread did not go on after it was read");
        #[cfg(all(windows, target_arch = "x86_64"))]
        {
            assert!(found.len() >= 3, "{found:?}");
            // Innermost, the system's own waiting, told by its name
            assert!(found[0].to_lowercase().starts_with("ntdll.dll!"), "{found:?}");
            // ...and further out, this program
            let me = std::env::current_exe().unwrap().file_name().unwrap().to_string_lossy().into_owned();
            assert!(found.iter().any(|f| f.starts_with(&me)), "this program is not in it: {found:?}");
            // No folder of anybody's is in a place's name
            assert!(!found.iter().any(|f| f.split('!').next().is_some_and(|file| file.contains('\\') || file.contains('/'))), "{found:?}");
        }
        #[cfg(not(all(windows, target_arch = "x86_64")))]
        assert!(found.is_empty());
        // A thread cannot hold itself still to be read
        assert!(of(this_thread()).is_empty());
    }

    /// A thread that is busy is read over and over and goes on every time:
    /// held once and let go once, however the reading and the waiting for
    /// it fall together
    #[test]
    fn a_busy_thread_goes_on_after_every_reading() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
        let (stop, turns) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicU64::new(0)));
        let (tx, rx) = std::sync::mpsc::channel::<u32>();
        let busy = {
            let (stop, turns) = (stop.clone(), turns.clone());
            std::thread::spawn(move || {
                let _ = tx.send(this_thread());
                let mut kept = Vec::new();
                while !stop.load(Ordering::Relaxed) {
                    // Allocating all the while, so it is often found holding the lock on the memory
                    kept.push(vec![0u8; 64]);
                    if kept.len() > 256 {
                        kept.clear();
                    }
                    turns.fetch_add(1, Ordering::Relaxed);
                }
            })
        };
        let thread = rx.recv().unwrap();
        for _ in 0..300 {
            let _ = places(thread);
        }
        let before = turns.load(Ordering::Relaxed);
        std::thread::sleep(std::time::Duration::from_millis(100));
        assert!(turns.load(Ordering::Relaxed) > before, "the thread was left held still");
        stop.store(true, Ordering::Relaxed);
        busy.join().unwrap();
        // A thread that is gone is not read, and nothing is the worse for asking
        assert!(places(thread).is_empty() || cfg!(all(windows, target_arch = "x86_64")));
    }
}
