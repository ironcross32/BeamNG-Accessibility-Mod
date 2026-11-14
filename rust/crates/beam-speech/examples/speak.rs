//! List Prism's backends with their capability bits and the chosen backend's voices,
//! then speak a line.
//!
//!     cargo run -p beam-speech --example speak -- --list
//!     cargo run -p beam-speech --example speak -- [--backend NAME] Text to speak
//!
//! Compare `--list` with `uv run python rust/fixtures/gen/speech_backends.py`.

use beam_speech::prism::Prism;
use beam_speech::speech;
use serde_json::json;

fn hex(bits: Option<u64>) -> String {
    bits.map_or("None".into(), |b| format!("0x{b:08x}"))
}

fn main() {
    let mut list_only = false;
    let mut backend = speech::AUTO_BACKEND.to_string();
    let mut words = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--list" => list_only = true,
            "--backend" => backend = args.next().unwrap_or(backend),
            _ => words.push(a),
        }
    }

    println!("prism ok: {}", speech::is_available());
    if let Some(p) = Prism::global() {
        println!("prism version: {}", p.version_string());
    }
    let backends = speech::list_backends();
    println!("list_backends: {backends:?}");

    // Each backend's own bits by direct acquire (no fallback), then what the speech
    // layer's list_voices reports for that name (which falls back to the best one).
    if let Some(p) = Prism::global() {
        match p.context(None) {
            Ok(ctx) => {
                for name in &backends {
                    let raw = match ctx.id_of_name(name).and_then(|id| ctx.acquire(id)) {
                        Ok(b) => format!(
                            "0x{:08x} name={:?}",
                            b.features().to_bits(),
                            b.name().unwrap_or_default()
                        ),
                        Err(e) => format!("unavailable ({e})"),
                    };
                    let (names, feats) = speech::list_voices(name);
                    println!("backend {name:?}: acquire {raw}");
                    println!(
                        "    list_voices -> {} voices, features {}",
                        names.len(),
                        hex(feats.map(|f| f.to_bits()))
                    );
                }
            }
            Err(e) => println!("could not create a probe context: {e}"),
        }
    }

    let (names, feats) = speech::list_voices(&backend);
    println!(
        "list_voices({backend:?}) -> features {}, voices {names:?}",
        hex(feats.map(|f| f.to_bits()))
    );
    let cfg = json!({"speech_backend": backend});
    println!("init({backend}): {}", speech::init(Some(&cfg)));
    println!("backend_name: {:?}", speech::backend_name());
    println!("describe_capabilities: {}", speech::describe_capabilities());

    if !list_only {
        let text = if words.is_empty() {
            "This is a test of the current speech configuration.".to_string()
        } else {
            words.join(" ")
        };
        println!("speak({text:?}): {}", speech::speak(&text, true));
        // speak() is asynchronous; give the synth time before the process exits.
        std::thread::sleep(std::time::Duration::from_secs(4));
    }
    speech::shutdown();
    speech::atexit();
}
