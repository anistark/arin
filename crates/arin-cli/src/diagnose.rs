//! Commands that answer questions about the machine rather than drawing on it.
//!
//! What this build can ground with, what displays are attached, whether the capture
//! permission is granted, and what a capture actually looks like. All read only.

use anyhow::Result;

// Everything below the resolver listing needs a screen to ask about, so on a platform
// with no renderer this module is `list_resolvers` and nothing else.
#[cfg(target_os = "macos")]
use anyhow::{Context, bail};
#[cfg(target_os = "macos")]
use arin_core::Config;
#[cfg(target_os = "macos")]
use arin_protocol::DisplayId;

/// Print what this build can ground queries with.
///
/// A separate command rather than a line in `--help`, because the set is a property of the
/// binary rather than of the invocation, and because whether a resolver leaves the machine
/// is the thing worth reading before choosing one.
pub(crate) fn list_resolvers() -> Result<()> {
    let registry = arin_resolve::Registry::with_builtins();
    if registry.is_empty() {
        println!("no resolvers are built into this binary");
        return Ok(());
    }
    println!("Pass one to `arin daemon --resolver <NAME>`. None is used unless you name it.\n");
    let mut local_failed = false;
    for name in registry.names() {
        // Built rather than described, so what is printed is what would actually run.
        // A resolver that cannot be constructed says why here rather than at first use.
        match registry.build(name) {
            Ok(resolver) if resolver.is_remote() => {
                println!("{name}\tready, and SENDS SCREENSHOTS OFF THIS MACHINE");
            }
            Ok(_) => println!("{name}\tready, and runs entirely on this machine"),
            Err(e) => {
                println!("{name}\tunavailable: {e}");
                local_failed |= name == "local";
            }
        }
    }
    if local_failed {
        local_resolver_hint();
    }
    Ok(())
}

/// What to do about a local resolver that did not build.
///
/// Printed alongside the list rather than as its own command, because the failure it
/// explains is almost never Arin's: somebody has no model server running, or has one on a
/// port other than the one guessed at. One line saying the resolver is unavailable does not
/// get anyone from there to a working daemon.
fn local_resolver_hint() {
    println!("\nThe local resolver looks for an OpenAI shaped chat completions endpoint on");
    println!("loopback. Ports the runtimes it speaks to use out of the box:\n");
    for (runtime, port) in arin_resolve::local::OTHER_PORTS {
        println!("  {port}\t{runtime}");
    }
    println!("\nPoint it at yours and load a UI TARS class grounding model:\n");
    println!("  export ARIN_LOCAL_ENDPOINT=http://127.0.0.1:11434/v1/chat/completions");
    println!("  export ARIN_LOCAL_MODEL=ui-tars-1.5-7b");
}

/// Print the displays the overlay would cover.
///
/// Needs the main thread for AppKit, but not the event loop, so it enumerates and exits.
/// This is how to find the id to pass to `--display`, since ids are the ones macOS
/// assigns rather than a count from one.
#[cfg(target_os = "macos")]
pub(crate) fn list_displays() -> Result<()> {
    let mtm = objc2::MainThreadMarker::new().context("must run on the main thread")?;
    for screen in arin_mac::known_screens(mtm) {
        let info = screen.info;
        println!(
            "{}\t{:.0}x{:.0} at {}x",
            info.id, info.logical_size[0], info.logical_size[1], info.scale
        );
    }
    Ok(())
}

/// Report whether screen capture works, and offer the way to fix it.
///
/// Separate from `arin status`, which asks whether the daemon is reachable. This asks
/// something else: a daemon with no permission is running perfectly, it just cannot see
/// the screen.
///
/// Proving the permission means taking a frame, and only one process can do that at a
/// time. So when the daemon is up it is the authority and this reports what it can check
/// without capturing, rather than taking its own failure to capture as a denial. Those
/// two look identical from here and mean opposite things.
#[cfg(target_os = "macos")]
pub(crate) fn check_permissions(config: &Config, open: bool) -> Result<()> {
    if open {
        if !arin_mac::open_screen_recording_settings() {
            bail!("could not open System Settings");
        }
        println!(
            "opened System Settings, now {}",
            arin_mac::SCREEN_RECORDING_HELP
        );
        return Ok(());
    }

    if std::os::unix::net::UnixStream::connect(&config.socket_path).is_ok() {
        if !arin_mac::screen_recording_granted() {
            println!("{}", arin_mac::ScreenRecording::Missing.explain());
            println!("`arin permissions --open` goes straight to the switch");
            std::process::exit(1);
        }
        println!("screen recording is granted");
        println!(
            "the daemon is running, and it is the one process that can take a frame. \
             Its log says on startup whether capture actually works. To prove it from \
             here, stop the daemon and run this again."
        );
        return Ok(());
    }

    // Capture must never run on the main thread.
    let state = std::thread::spawn(arin_mac::screen_recording)
        .join()
        .map_err(|_| anyhow::anyhow!("the permission check panicked"))?;

    println!("{}", state.explain());

    if state != arin_mac::ScreenRecording::Working {
        println!("`arin permissions --open` goes straight to the switch");
        std::process::exit(1);
    }
    Ok(())
}

/// Capture one frame and describe it.
///
/// Runs off the main thread on purpose. Capture blocks until ScreenCaptureKit answers,
/// and its handlers want a thread that is not sitting in a join.
#[cfg(target_os = "macos")]
pub(crate) fn capture_once(
    display: u32,
    probe: Option<String>,
    save: Option<&std::path::Path>,
) -> Result<()> {
    use arin_core::Capture as _;

    // Two frames a moment apart, so the report says not just what one looks like but how
    // much a still screen drifts between captures. That number is what scroll detection
    // has to see past.
    let (frame, second) = std::thread::spawn(move || {
        // Full resolution, which is also what a corpus wants: the daemon downscales for
        // scroll detection because it reads coarse statistics twice a second, and a corpus
        // built from thumbnails would cap every experiment ever run against it.
        let backend = arin_mac::MacCapture::default();
        let first = backend.capture(DisplayId(display))?;
        std::thread::sleep(std::time::Duration::from_millis(400));
        let second = backend.capture(DisplayId(display))?;
        Ok::<_, arin_core::Error>((first, second))
    })
    .join()
    .map_err(|_| anyhow::anyhow!("the capture thread panicked"))??;

    let expected = frame.width as usize * frame.height as usize * 4;
    println!("display     {}", frame.display);
    println!("physical    {}x{}", frame.width, frame.height);
    println!(
        "logical     {:.0}x{:.0} at {}x",
        frame.logical_size[0], frame.logical_size[1], frame.scale
    );
    println!("bytes       {} (expected {})", frame.pixels.len(), expected);
    let drift = frame.signature().drift(&second.signature());
    println!(
        "drift       {:.3}% of cells over 400ms on a still screen ({})",
        drift * 100.0,
        if second.signature().moved_from(&frame.signature()) {
            "reads as movement"
        } else {
            "reads as still"
        }
    );

    let non_zero = frame.pixels.iter().filter(|b| **b != 0).count();
    println!(
        "non zero    {non_zero} of {} bytes ({:.1}%)",
        frame.pixels.len(),
        100.0 * non_zero as f64 / frame.pixels.len().max(1) as f64
    );

    if let Some(probe) = probe {
        let (x, y) = probe
            .split_once(',')
            .context("probe wants `x,y` in logical points")?;
        let x: f64 = x.trim().parse().context("probe x")?;
        let y: f64 = y.trim().parse().context("probe y")?;
        let px = (x * frame.scale) as usize;
        let py = (y * frame.scale) as usize;
        let idx = (py * frame.width as usize + px) * 4;
        match frame.pixels.get(idx..idx + 4) {
            Some(p) => println!(
                "probe       logical {x},{y} -> physical {px},{py} = [{}, {}, {}, {}]",
                p[0], p[1], p[2], p[3]
            ),
            None => println!("probe       logical {x},{y} is outside the frame"),
        }
    }

    if let Some(dir) = save {
        // Numbered from what is already there, so repeated captures build one corpus
        // rather than overwriting each other.
        let stem = format!("{:03}-display{display}", next_index(dir));
        let written = arin_resolve::eval::save_frame(dir, &stem, &frame)
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        println!("\nsaved       {}", dir.join(&stem).display());
        for path in &written {
            println!(
                "  {}",
                path.file_name().unwrap_or_default().to_string_lossy()
            );
        }
        println!(
            "\nOpen the png, read off where each target is in image pixels, and add it to\n\
             {}. Then score a resolver against it:\n  \
             cargo run --release -p arin-resolve --example eval -- {} local",
            dir.join("cases.json").display(),
            dir.display()
        );
    }

    Ok(())
}

/// The next free number in a corpus directory.
///
/// Counting manifests rather than tracking state, so a directory built over several
/// sessions keeps going up instead of overwriting what is already there.
#[cfg(target_os = "macos")]
fn next_index(dir: &std::path::Path) -> usize {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(|entry| entry.ok())
                .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "json"))
                .filter(|entry| entry.file_name() != "cases.json")
                .count()
        })
        .unwrap_or(0)
}
