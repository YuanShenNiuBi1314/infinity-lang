//! STUB CLI — §13.4. The public API below is frozen.

/// Runs one `infy …` invocation. `args[0]` is the program name.
/// Returns a §13.4 exit code: 0 ok · 1 generic · 2 diagnostics blocked · 3 runtime panic ·
/// 4 permission/cache · 5 network/dependency · 6 toolchain/link · 7 checksum/reproducibility.
pub fn run(args: &[String]) -> i32 {
    let _ = args;
    1
}
