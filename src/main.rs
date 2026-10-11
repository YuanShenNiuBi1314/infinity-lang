//! `infy` entry point (§13.4). Exit codes are normative: see [`crate::cli`].

fn main() -> ! {
    let args: Vec<String> = std::env::args().collect();
    std::process::exit(infy::cli::run(&args))
}
