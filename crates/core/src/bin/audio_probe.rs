//! Is the sound of one page really what comes out, and nothing else?
//!
//!     cargo run -p shikisha-core --bin audio_probe -- <a process id> [seconds]
//!
//! Prints how loud what it heard was, a line a second. Play something in that
//! process and the numbers rise; play something in ANOTHER program and they
//! must not. That second half is the point: a screen somebody chose to share
//! is not permission to listen to the room.
//!
//! The process to name is the browser the page lives in, not this program.
//! `Get-Process msedgewebview2` lists them; the one with the biggest working
//! set is usually the browser itself, and its children are included anyway.
fn main() {
    let mut args = std::env::args().skip(1);
    let Some(pid) = args.next().and_then(|a| a.parse::<u32>().ok()) else {
        eprintln!("say which process: audio_probe <pid> [seconds]");
        std::process::exit(2);
    };
    let secs: u64 = args.next().and_then(|a| a.parse().ok()).unwrap_or(10);

    let mut ears = match shikisha_core::vaudio::listen(pid) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("{e:#}");
            std::process::exit(1);
        }
    };
    println!("listening to {pid} and its children for {secs}s");

    let began = std::time::Instant::now();
    let mut second = std::time::Instant::now();
    let (mut loudest, mut samples) = (0.0f32, 0usize);
    while began.elapsed().as_secs() < secs {
        match ears.take() {
            Ok(got) => {
                samples += got.len();
                for s in got {
                    loudest = loudest.max(s.abs());
                }
            }
            Err(e) => {
                eprintln!("the sound stopped: {e:#}");
                break;
            }
        }
        if second.elapsed() >= std::time::Duration::from_secs(1) {
            let heard = samples / shikisha_core::vaudio::CHANNELS;
            println!(
                "  loudest {:.3}  {} samples a channel ({:.2}s of sound)",
                loudest,
                heard,
                heard as f64 / shikisha_core::vaudio::RATE as f64
            );
            loudest = 0.0;
            samples = 0;
            second = std::time::Instant::now();
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}
