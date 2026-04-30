use anyhow::{Context, Result};
use std::path::PathBuf;
use std::time::Duration;

fn main() -> Result<()> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let title = arg_value(&args, "--title")
        .unwrap_or("Boon app_window smoke")
        .to_string();
    let hold_ms = arg_value(&args, "--hold-ms")
        .unwrap_or("0")
        .parse::<u64>()
        .context("--hold-ms must be an integer")?;
    let out = arg_value(&args, "--out")
        .map(PathBuf::from)
        .context("--out <path> is required")?;
    boon_backend_app_window::smoke_test_helper_main(title, Duration::from_millis(hold_ms), out);
}

fn arg_value<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.windows(2)
        .find(|pair| pair[0] == name)
        .map(|pair| pair[1].as_str())
}
