//! One of the helper processes of the Chromium inside a Mac's SHIKISHA-TERM.app.
//!
//! Chromium draws pages in processes of their own -- a renderer for the pages,
//! the GPU, the network -- and starts each from a helper app inside the
//! `.app`, under the name a Mac's code signing wants for that kind
//! (`SHIKISHA-TERM Helper (Renderer).app` and the rest). This is that program.
//! It is told what to be on its command line, does Chromium's part and ends:
//! nothing of SHIKISHA-TERM's own runs here.

#[cfg(target_os = "macos")]
fn main() {
    use cef::args::Args;

    // Where Chromium's framework is, from a helper's program: the helper app
    // sits in the main app's Contents/Frameworks, and so does the framework
    const FROM_HELPER: &str = "../../../Chromium Embedded Framework.framework";

    let Some(exe) = std::env::current_exe().ok() else { std::process::exit(1) };
    let Some(framework) = exe.parent().map(|p| p.join(FROM_HELPER)) else { std::process::exit(1) };
    if !framework.exists() {
        eprintln!("this is a helper of the Chromium inside SHIKISHA-TERM.app, and only runs from there");
        std::process::exit(1);
    }
    let args = Args::new();
    // A renderer is put in the system's sandbox before anything of a page
    // reaches it
    let _sandbox = {
        let mut sandbox = cef::sandbox::Sandbox::new();
        sandbox.initialize(args.as_main_args());
        sandbox
    };
    let loader = cef::library_loader::LibraryLoader::new(&exe, true);
    if !loader.load() {
        std::process::exit(1);
    }
    let _ = cef::api_hash(cef::sys::CEF_API_VERSION_LAST, 0);
    let code = cef::execute_process(Some(args.as_main_args()), None::<&mut cef::App>, std::ptr::null_mut());
    std::process::exit(code);
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("this is a helper of the Chromium inside a Mac's SHIKISHA-TERM.app; there is nothing for it to do here");
    std::process::exit(1);
}
