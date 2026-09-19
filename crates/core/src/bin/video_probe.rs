//! Does the whole video path actually reach a browser?
//!
//! Every piece of it has its own tests, and they prove the pieces. What they
//! cannot prove is the thing that matters: that a browser, at the other end
//! of a real connection, decodes what this machine sends. Between here and
//! there are a codec agreement, a packetiser, an encoder written against an
//! operating system's interface, and a phone's idea of what it will play --
//! and every one of them can be right on its own and wrong together.
//!
//!   cargo run -p shikisha-core --bin video_probe -- <folder of .jpg> [seconds]
//!
//! It stands up the real relay, pushes the pictures in that folder through
//! the same door the browser relay pushes them through
//! ([`shikisha_core::remote::RemoteUi::push_frame`]), and prints the address
//! and token to point a viewer at. `tools/debug/video-endtoend.mjs` collects
//! the pictures, drives the viewer and reads the verdict; this half is here so
//! that what is measured is the product's own code and not a copy of it.

use std::time::{Duration, Instant};

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(dir) = args.next() else {
        eprintln!("say which folder the pictures are in");
        std::process::exit(2);
    };
    let secs: u64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(30);

    let mut frames: Vec<Vec<u8>> = Vec::new();
    let mut names: Vec<_> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("{dir} cannot be read: {e}"))
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "jpg"))
        .collect();
    names.sort();
    for p in &names {
        frames.push(std::fs::read(p).expect("a picture went away"));
    }
    if frames.is_empty() {
        eprintln!("there are no pictures in {dir}");
        std::process::exit(2);
    }

    let token = "video-probe-token".to_string();
    let ui = shikisha_core::remote::RemoteUi::start(
        "127.0.0.1".parse().unwrap(),
        0,
        token.clone(),
        String::new(),
    )
    .expect("the relay would not start");

    // What one picture costs, before anybody is watching. If this is over a
    // thirtieth of a second the ceiling is a wish: the pictures cannot be
    // prepared as fast as they are asked for, and the far end sees the
    // difference as dropped frames
    {
        let one = &frames[0];
        let began = Instant::now();
        let planes = shikisha_core::vframe::planes_of(one).expect("the picture will not read");
        let undo = began.elapsed();
        let mut enc = shikisha_core::vencode::encoder_for(planes.width, planes.height, 30)
            .expect("no encoder");
        // The first few are the encoder settling; the ones after it are the cost
        for _ in 0..5 {
            let _ = enc.encode(&planes, false);
        }
        let began = Instant::now();
        let mut n = 0;
        while began.elapsed() < Duration::from_secs(2) {
            let p = shikisha_core::vframe::planes_of(&frames[n % frames.len()]).unwrap();
            let _ = enc.encode(&p, false);
            n += 1;
        }
        let each = began.elapsed().as_secs_f64() / n as f64;
        println!(
            "one picture {}x{}: undoing it {:.1}ms, the whole way {:.1}ms = {:.0} a second",
            planes.width,
            planes.height,
            undo.as_secs_f64() * 1000.0,
            each * 1000.0,
            1.0 / each
        );
    }

    println!("origin {}", ui.url.split("/?").next().unwrap());
    println!("token {token}");
    println!("frames {}", frames.len());
    println!("encoder {}", shikisha_core::vencode::codec());
    println!("ready");

    // Sixty a second, which is what the relay was measured producing. What
    // reaches the far end is whatever the pacing decides to keep
    let began = Instant::now();
    let mut n = 0usize;
    while began.elapsed() < Duration::from_secs(secs) {
        ui.push_frame(frames[n % frames.len()].clone());
        n += 1;
        std::thread::sleep(Duration::from_millis(16));
    }
    println!("pushed {n}");
    println!("watching {}", ui.casts());
}
