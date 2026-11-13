//! Build, package and bundle BEAM: the port of build.py (workstream R1).
//!
//! Run as `cargo run -p xtask -- <command>`.

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None | Some("help") | Some("--help") => {
            println!("usage: cargo run -p xtask -- <command>");
            println!("no commands yet: build.py is ported by workstream R1");
        }
        Some(other) => {
            eprintln!("xtask: unknown command {other:?}");
            std::process::exit(2);
        }
    }
}
