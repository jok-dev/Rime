//! Peak-RSS measurement for a full-corpus parse.
//!
//! The likeliest disqualifier for full-moon is memory, not speed: a lossless
//! CST with trivia attached is much fatter than Luau's arena AST, and the tree
//! is RETAINED (we need it for pragmas and for LSP-quality incremental work).
//!
//! Threshold: > 1GB peak RSS on a large repo -> reconsider (see benches/parse.rs).
//!
//! Reports the RSS high-water mark rather than the instantaneous value, so it
//! survives the drop at the end: VmHWM from /proc/self/status on Linux,
//! PeakWorkingSetSize from K32GetProcessMemoryInfo on Windows.

use std::path::PathBuf;

#[cfg(target_os = "linux")]
fn peak_rss_kb() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("VmHWM:") {
            return rest.trim().trim_end_matches(" kB").trim().parse().ok();
        }
    }
    None
}

#[cfg(windows)]
fn peak_rss_kb() -> Option<u64> {
    use windows_sys::Win32::System::ProcessStatus::{
        K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    unsafe {
        let mut pmc: PROCESS_MEMORY_COUNTERS = std::mem::zeroed();
        pmc.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
        if K32GetProcessMemoryInfo(GetCurrentProcess(), &mut pmc, pmc.cb) == 0 {
            return None;
        }
        Some(pmc.PeakWorkingSetSize as u64 / 1024)
    }
}

#[cfg(not(any(target_os = "linux", windows)))]
fn peak_rss_kb() -> Option<u64> {
    None
}

fn main() {
    let dir = std::env::args().nth(1).unwrap_or_else(|| {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../corpus")
            .to_string_lossy()
            .into_owned()
    });

    let files: Vec<String> = walkdir::WalkDir::new(&dir)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().is_some_and(|x| x == "luau" || x == "lua"))
        .filter_map(|e| std::fs::read_to_string(e.path()).ok())
        .collect();

    let bytes: usize = files.iter().map(String::len).sum();
    let baseline = peak_rss_kb().unwrap_or(0);

    let start = std::time::Instant::now();
    // RETAIN every tree — this is the whole point of the measurement.
    let trees: Vec<_> = files.iter().filter_map(|s| full_moon::parse(s).ok()).collect();
    let elapsed = start.elapsed();

    let peak = peak_rss_kb().unwrap_or(0);
    println!("files parsed : {}", trees.len());
    println!("source bytes : {:.1} MiB", bytes as f64 / 1048576.0);
    println!("wall clock   : {:.3} s", elapsed.as_secs_f64());
    println!("throughput   : {:.1} MiB/s", (bytes as f64 / 1048576.0) / elapsed.as_secs_f64());
    println!("baseline RSS : {:.1} MiB", baseline as f64 / 1024.0);
    println!("peak RSS     : {:.1} MiB", peak as f64 / 1024.0);
    println!("CST overhead : {:.1}x source size", ((peak - baseline) as f64 * 1024.0) / bytes as f64);

    std::hint::black_box(&trees);
}
