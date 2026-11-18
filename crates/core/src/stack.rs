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
//! A build made for trying things out carries the names, and says them.

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
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, HMODULE};
    use windows_sys::Win32::System::Diagnostics::Debug::{CONTEXT, GetThreadContext, ReadProcessMemory, RtlLookupFunctionEntry, RtlVirtualUnwind, UNW_FLAG_NHANDLER};
    use windows_sys::Win32::System::LibraryLoader::{GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, GetModuleFileNameW, GetModuleHandleExW};
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenThread, ResumeThread, SuspendThread, THREAD_GET_CONTEXT, THREAD_QUERY_INFORMATION, THREAD_SUSPEND_RESUME};

    /// The registers that say where a thread is and how to step out of it
    const WHERE: u32 = 0x0010_0003;
    /// How long the reading may take before the thread is let go on without it
    const GIVEN: std::time::Duration = std::time::Duration::from_millis(1500);

    // Who lets the thread go on: nobody yet, the one reading, or the one
    // who asked and stopped waiting
    const READING: u8 = 0;
    const READ: u8 = 1;
    const GIVEN_UP: u8 = 2;

    /// The thread is held still while it is read, and a thread held still
    /// keeps whatever it holds -- the lock on the memory everybody
    /// allocates from, for one. So nothing is allocated while it is held:
    /// what is needed is made before. And the reading is done by a thread
    /// of its own, which whoever asked waits for only so long: if the held
    /// thread has the one lock the reading itself needs, the reader would
    /// wait for it for ever, with the thread it is waiting for held still
    /// by itself. Then whoever asked lets the thread go on, and does without
    pub fn places(thread: u32) -> Vec<u64> {
        // SAFETY: a thread of this process, opened to be held and read; the handle is closed below
        let handle = unsafe { OpenThread(THREAD_SUSPEND_RESUME | THREAD_GET_CONTEXT | THREAD_QUERY_INFORMATION, 0, thread) };
        if handle.is_null() || thread == super::this_thread() {
            return Vec::new();
        }
        let handle = handle as usize;
        let who = Arc::new(AtomicU8::new(READING));
        let (tx, rx) = std::sync::mpsc::channel::<Vec<u64>>();
        let reader = {
            let who = who.clone();
            std::thread::Builder::new().name("shikisha-stack".into()).spawn(move || {
                let mut found = Vec::with_capacity(MOST);
                // SAFETY: zeroes are a context to be filled in, and the box keeps it lined up as the system wants
                let mut context: Box<CONTEXT> = Box::new(unsafe { std::mem::zeroed() });
                context.ContextFlags = WHERE;
                let handle = handle as HANDLE;
                // SAFETY: the handle is open; from here to the resume nothing is allocated
                unsafe {
                    if SuspendThread(handle) == u32::MAX {
                        let _ = tx.send(found);
                        return;
                    }
                    if GetThreadContext(handle, &mut *context) != 0 {
                        while found.len() < MOST && who.load(Ordering::SeqCst) == READING {
                            let at = context.Rip;
                            if at == 0 {
                                break;
                            }
                            found.push(at);
                            let mut base = 0u64;
                            let entry = RtlLookupFunctionEntry(at, &mut base, std::ptr::null_mut());
                            if entry.is_null() {
                                // A function that keeps nothing on the stack: where it goes back to is on top
                                let (mut back, mut got) = (0u64, 0usize);
                                if ReadProcessMemory(GetCurrentProcess(), context.Rsp as *const _, &mut back as *mut u64 as *mut _, 8, &mut got) == 0 || got != 8 {
                                    break;
                                }
                                context.Rip = back;
                                context.Rsp += 8;
                            } else {
                                let (mut data, mut frame) = (std::ptr::null_mut(), 0u64);
                                RtlVirtualUnwind(UNW_FLAG_NHANDLER, base, at, entry, &mut *context, &mut data, &mut frame, std::ptr::null_mut());
                            }
                        }
                    }
                    if who.compare_exchange(READING, READ, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
                        ResumeThread(handle);
                    }
                }
                let _ = tx.send(found);
            })
        };
        if reader.is_err() {
            // SAFETY: opened above
            unsafe { CloseHandle(handle as HANDLE) };
            return Vec::new();
        }
        let found = rx.recv_timeout(GIVEN).unwrap_or_default();
        if who.compare_exchange(READING, GIVEN_UP, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
            // The reader is held up: the thread goes on, and the reader stops at its next step.
            // The handle is left open for it
            // SAFETY: opened above
            unsafe { ResumeThread(handle as HANDLE) };
            return Vec::new();
        }
        // SAFETY: opened above, and the reader is done with it
        unsafe { CloseHandle(handle as HANDLE) };
        found
    }

    /// A value out of a file of the program as it is laid out in memory
    ///
    /// SAFETY: `base + at` has to be inside a file the system has laid out
    unsafe fn read<T: Copy>(base: u64, at: u64) -> T {
        unsafe { std::ptr::read_unaligned((base + at) as *const T) }
    }

    /// The function of a system's file that `place` is in, by the names the
    /// file itself gives out, and how far into it
    fn named(base: u64, place: u64) -> Option<(String, u64)> {
        // SAFETY: `base` is where the system laid a file out; every place read is checked against the file's own size
        unsafe {
            if read::<u16>(base, 0) != 0x5a4d {
                return None;
            }
            let head = read::<u32>(base, 0x3c) as u64;
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
            if count > 100_000 || names + count * 4 > size || numbers + count * 2 > size || functions + all * 4 > size {
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
        // SAFETY: asked by a place, and the file's count of users left as it is
        let found = unsafe { GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, place as *const u16, &mut module) };
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
        #[cfg(debug_assertions)]
        if let Some(said) = symbols::of(place, base, &name[..len.min(name.len())]) {
            return format!("{file}!{said}");
        }
        match named(base, at) {
            Some((function, into)) => format!("{file}!{function}+{into:#x}"),
            None => format!("{file}+{at:#x}"),
        }
    }

    /// In a build made for trying things out, the names of this program's
    /// own functions are beside it, and Windows' own reader of them says
    /// which one a place is in
    #[cfg(debug_assertions)]
    mod symbols {
        use windows_sys::Win32::System::Diagnostics::Debug::{
            IMAGEHLP_LINEW64, SYMBOL_INFOW, SYMOPT_DEFERRED_LOADS, SYMOPT_LOAD_LINES, SYMOPT_UNDNAME, SymFromAddrW, SymGetLineFromAddrW64, SymInitializeW, SymLoadModuleExW, SymSetOptions,
        };
        use windows_sys::Win32::System::Threading::GetCurrentProcess;

        /// The reader is not one two threads may use at once. With it, whether
        /// it has been started, and the files it has been told of
        static TURN: std::sync::Mutex<(bool, Vec<u64>)> = std::sync::Mutex::new((false, Vec::new()));

        /// The function `place` is in, with how far in and the line of the
        /// source; `base` and `file` are the file of the program it is in
        pub fn of(place: u64, base: u64, file: &[u16]) -> Option<String> {
            let mut turn = TURN.lock().unwrap_or_else(|e| e.into_inner());
            let (ready, told) = &mut *turn;
            // SAFETY: this process, and room of the size said
            unsafe {
                let process = GetCurrentProcess();
                if !*ready {
                    SymSetOptions(SYMOPT_UNDNAME | SYMOPT_LOAD_LINES);
                    // Each file is told to it as it comes up, rather than all of them at once
                    if SymInitializeW(process, std::ptr::null(), 0) == 0 {
                        return None;
                    }
                    *ready = true;
                }
                if !told.contains(&base) {
                    told.push(base);
                    let path: Vec<u16> = file.iter().cloned().chain(std::iter::once(0)).collect();
                    SymLoadModuleExW(process, std::ptr::null_mut(), path.as_ptr(), std::ptr::null(), base, 0, std::ptr::null(), 0);
                }
                const NAME: usize = 512;
                let mut room = vec![0u64; (std::mem::size_of::<SYMBOL_INFOW>() + NAME * 2) / 8 + 1];
                let symbol = room.as_mut_ptr() as *mut SYMBOL_INFOW;
                (*symbol).SizeOfStruct = std::mem::size_of::<SYMBOL_INFOW>() as u32;
                (*symbol).MaxNameLen = NAME as u32;
                let mut into = 0u64;
                if SymFromAddrW(process, place, &mut into, symbol) == 0 {
                    return None;
                }
                let name = String::from_utf16_lossy(std::slice::from_raw_parts((*symbol).Name.as_ptr(), ((*symbol).NameLen as usize).min(NAME)));
                let mut line: IMAGEHLP_LINEW64 = std::mem::zeroed();
                line.SizeOfStruct = std::mem::size_of::<IMAGEHLP_LINEW64>() as u32;
                let mut across = 0u32;
                let at = match SymGetLineFromAddrW64(process, place, &mut across, &mut line) != 0 && !line.FileName.is_null() {
                    true => {
                        let mut len = 0;
                        while *line.FileName.add(len) != 0 {
                            len += 1;
                        }
                        let file = String::from_utf16_lossy(std::slice::from_raw_parts(line.FileName, len));
                        format!(" ({}:{})", file.rsplit(['\\', '/']).next().unwrap_or(&file), line.LineNumber)
                    }
                    false => String::new(),
                };
                Some(format!("{name}+{into:#x}{at}"))
            }
        }
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
}
