use anyhow::{Context, Result, bail};
use boon_codegen_rust::{generate_manifest, generate_program_spec};
use boon_examples::list_examples;
use boon_verify::{
    run_native_app_window_example, run_native_playground, verify_all, verify_browser_firefox,
    verify_native_app_window, verify_native_wgpu_headless, verify_ratatui,
};
use serde_json::json;
use std::env;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;
use walkdir::WalkDir;
use wesl::Wesl;
use wgsl_bindgen::{WgslBindgenOptionBuilder, WgslShaderSourceType, WgslTypeSerializeStrategy};

fn main() -> Result<()> {
    let mut args = env::args().skip(1).collect::<Vec<_>>();
    if args.first().is_some_and(|arg| arg == "xtask") {
        args.remove(0);
    }
    match args.first().map(String::as_str) {
        Some("examples") if args.get(1).map(String::as_str) == Some("list") => examples_list(),
        Some("bootstrap") => bootstrap(args.iter().any(|arg| arg == "--check")),
        Some("generate") => generate(),
        Some("shaders") => shaders(),
        Some("verify") => verify(&args[1..]),
        Some("doctor") if args.get(1).map(String::as_str) == Some("firefox-webgpu") => {
            boon_backend_browser::doctor_firefox_webgpu()
                .map(|cap| println!("{}", serde_json::to_string_pretty(&cap).unwrap()))
        }
        Some("firefox") if args.get(1).map(String::as_str) == Some("install-native-host") => {
            install_native_host(false)
        }
        Some("firefox") if args.get(1).map(String::as_str) == Some("reset-profile") => {
            let profile = repo_root()?.join(".boon-local/firefox-profile");
            if profile.exists() {
                fs::remove_dir_all(&profile)?;
            }
            println!("removed {}", profile.display());
            Ok(())
        }
        Some("run") if args.get(1).map(String::as_str) == Some("native") => run_native(&args[2..]),
        Some("run") if args.get(1).map(String::as_str) == Some("ratatui") => {
            run_ratatui(&args[2..])
        }
        Some("run") if args.get(1).map(String::as_str) == Some("browser") => {
            run_browser(&args[2..])
        }
        Some("playground") if args.get(1).map(String::as_str) == Some("native") => {
            playground_native(&args[2..])
        }
        Some("bench") => bench(&args[1..]),
        _ => {
            eprintln!(
                "commands: examples list | bootstrap [--check] | generate | shaders | verify <all|ratatui|native-wgpu|browser-wgpu> | run <native|ratatui|browser> --example <name> [--hold-ms <ms>] | playground native [--example <name>] [--hold-ms <ms>] | doctor firefox-webgpu | firefox reset-profile"
            );
            bail!("unknown xtask command")
        }
    }
}

fn examples_list() -> Result<()> {
    for name in list_examples() {
        println!("{name}");
    }
    Ok(())
}

fn bootstrap(check: bool) -> Result<()> {
    let root = repo_root()?;
    let local = root.join(".boon-local");
    let tools = local.join("tools");
    let profile = local.join("firefox-profile");
    fs::create_dir_all(&tools)?;
    fs::create_dir_all(&profile)?;
    fs::write(
        profile.join("user.js"),
        "user_pref(\"dom.webgpu.enabled\", true);\n",
    )?;

    if command_exists("rustup") {
        if check {
            let installed =
                command_stdout(Command::new("rustup").args(["target", "list", "--installed"]))?;
            if !installed
                .lines()
                .any(|line| line.trim() == "wasm32-unknown-unknown")
            {
                bail!(
                    "missing Rust target wasm32-unknown-unknown; run `rustup target add wasm32-unknown-unknown`"
                );
            }
        } else {
            run(Command::new("rustup").args(["target", "add", "wasm32-unknown-unknown"]))?;
        }
    }

    let web_ext = tools.join("node_modules/.bin/web-ext");
    let web_ext_version = repo_local_web_ext_version(&tools)?;
    if !web_ext.exists() || web_ext_version.as_deref() != Some("10.1.0") {
        if check {
            bail!("missing or wrong repo-local web-ext@10.1.0; run `cargo xtask bootstrap`");
        }
        if !command_exists("npm") {
            bail!(
                "missing npm needed for repo-local web-ext; install Node/npm, for example `sudo apt install nodejs npm`"
            );
        }
        run(Command::new("npm").current_dir(&root).args([
            "--prefix",
            ".boon-local/tools",
            "install",
            "web-ext@10.1.0",
        ]))?;
    }

    if !command_exists("firefox") {
        bail!(
            "missing stable Firefox; install it with `sudo apt install firefox` or your distribution equivalent"
        );
    }

    install_native_host(check)?;
    browser_wasm(check)?;

    println!("bootstrap ok: {}", local.display());
    Ok(())
}

fn browser_wasm(check: bool) -> Result<()> {
    let root = repo_root()?;
    let wasm = root.join("target/wasm32-unknown-unknown/release/boon_browser_runner.wasm");
    if check {
        if !wasm.exists() {
            bail!(
                "missing browser wasm runner {}; run `cargo build -p boon_browser_runner --release --target wasm32-unknown-unknown`",
                wasm.display()
            );
        }
    } else {
        run(Command::new("cargo").current_dir(&root).args([
            "build",
            "-p",
            "boon_browser_runner",
            "--release",
            "--target",
            "wasm32-unknown-unknown",
        ]))?;
    }
    if !wasm.exists() {
        bail!("expected browser wasm runner at {}", wasm.display());
    }
    Ok(())
}

fn install_native_host(check: bool) -> Result<()> {
    let root = repo_root()?;
    let host_binary = root.join("target/debug/boon-firefox-native-host");
    if check {
        if !host_binary.exists() {
            bail!(
                "missing Firefox native host binary; run `cargo xtask firefox install-native-host`"
            );
        }
    } else {
        run(Command::new("cargo").current_dir(&root).args([
            "build",
            "-p",
            "boon_verify",
            "--bin",
            "boon-firefox-native-host",
        ]))?;
    }
    if !host_binary.exists() {
        bail!("expected native host binary at {}", host_binary.display());
    }

    let manifest_dir = firefox_native_host_manifest_dir()?;
    let manifest_path = manifest_dir.join("boon_firefox_native_host.json");
    let manifest = json!({
        "name": "boon_firefox_native_host",
        "description": "Boon Rust Firefox WebGPU verification native host",
        "path": host_binary,
        "type": "stdio",
        "allowed_extensions": ["boon-rust-test@boonlang.local"],
    });
    let manifest_bytes = serde_json::to_vec_pretty(&manifest)?;
    let stale = fs::read(&manifest_path).map_or(true, |old| old != manifest_bytes);
    if stale {
        if check {
            bail!(
                "Firefox native messaging manifest is missing or stale at {}; run `cargo xtask firefox install-native-host`",
                manifest_path.display()
            );
        }
        fs::create_dir_all(&manifest_dir)?;
        fs::write(&manifest_path, manifest_bytes)?;
    }

    ping_native_host(&host_binary)?;
    println!(
        "installed Firefox native host manifest {}",
        manifest_path.display()
    );
    Ok(())
}

fn firefox_native_host_manifest_dir() -> Result<PathBuf> {
    if cfg!(target_os = "linux") {
        Ok(home_dir()?.join(".mozilla/native-messaging-hosts"))
    } else if cfg!(target_os = "macos") {
        Ok(home_dir()?.join("Library/Application Support/Mozilla/NativeMessagingHosts"))
    } else {
        bail!(
            "unsupported Firefox native host installer platform; install the manifest manually for this OS"
        )
    }
}

fn ping_native_host(host_binary: &Path) -> Result<()> {
    let mut child = Command::new(host_binary)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .with_context(|| format!("spawning native host {}", host_binary.display()))?;
    let request = serde_json::to_vec(&json!({"type": "ping"}))?;
    {
        let stdin = child.stdin.as_mut().context("native host stdin")?;
        stdin.write_all(&(request.len() as u32).to_le_bytes())?;
        stdin.write_all(&request)?;
    }
    let mut len = [0u8; 4];
    let stdout = child.stdout.as_mut().context("native host stdout")?;
    stdout.read_exact(&mut len)?;
    let len = u32::from_le_bytes(len) as usize;
    let mut response = vec![0u8; len];
    stdout.read_exact(&mut response)?;
    let response: serde_json::Value = serde_json::from_slice(&response)?;
    let status = child.wait()?;
    if !status.success() {
        bail!("native host ping exited with {status}");
    }
    if response.get("type").and_then(|v| v.as_str()) != Some("pong") {
        bail!("native host ping returned unexpected response: {response}");
    }
    Ok(())
}

fn home_dir() -> Result<PathBuf> {
    env::var_os("HOME")
        .map(PathBuf::from)
        .context("HOME is not set; cannot install Firefox native messaging manifest")
}

fn generate() -> Result<()> {
    let root = repo_root()?;
    let mut app_sources = Vec::new();
    for name in list_examples() {
        let src = root.join("examples").join(name).join("source.bn");
        let out = root
            .join("examples")
            .join(name)
            .join("expected.source_inventory.json");
        generate_manifest(name, &src, &out).with_context(|| format!("generating {name}"))?;
        let out = root
            .join("examples")
            .join(name)
            .join("expected.program.json");
        generate_program_spec(name, &src, &out).with_context(|| format!("generating {name}"))?;
        app_sources.push((*name, src));
    }
    let generated = root.join("target/generated-examples");
    fs::create_dir_all(&generated)?;
    boon_codegen_rust::generate_examples_module(
        &app_sources,
        &generated.join("generated_examples.rs"),
    )?;
    Ok(())
}

fn shaders() -> Result<()> {
    let root = repo_root()?;
    let generated = root.join("target/generated-shaders");
    fs::create_dir_all(&generated)?;
    let shader_root = root.join("shaders");
    let compiler = Wesl::new(&shader_root);
    let mut generated_wgsl = Vec::new();
    for root_name in ["ui_rects", "ui_text", "grid", "physical_debug", "present"] {
        let wesl_root = format!("package::pipelines::{root_name}");
        let compiled = compiler
            .compile(&wesl_root.parse()?)
            .map_err(|err| anyhow::anyhow!("WESL compile failed for {wesl_root}: {err}"))?;
        let wgsl = generated.join(format!("{root_name}.wgsl"));
        fs::write(&wgsl, compiled.to_string())?;
        generated_wgsl.push(wgsl);
    }
    let mut builder = WgslBindgenOptionBuilder::default();
    builder
        .workspace_root(&generated)
        .output(generated.join("bindings.rs"))
        .serialization_strategy(WgslTypeSerializeStrategy::Encase)
        .shader_source_type(WgslShaderSourceType::EmbedSource);
    for wgsl in &generated_wgsl {
        builder.add_entry_point(wgsl.to_string_lossy().to_string());
    }
    builder.build()?.generate()?;
    println!(
        "compiled WESL roots to WGSL and wgsl_bindgen bindings in {}",
        generated.display()
    );
    Ok(())
}

fn verify(args: &[String]) -> Result<()> {
    let root = repo_root()?;
    let artifacts = root.join("target/boon-artifacts");
    fs::create_dir_all(&artifacts)?;
    let success_path = artifacts.join("success.json");
    if args.first().map(String::as_str) == Some("all") && success_path.exists() {
        fs::remove_file(&success_path)?;
    }
    let no_bootstrap = args.iter().any(|arg| arg == "--no-bootstrap");
    if !no_bootstrap {
        bootstrap(false)?;
    }
    generate()?;
    shaders()?;
    if args.first().map(String::as_str) == Some("all") {
        quality_gates(&root)?;
    }

    let report = match args.first().map(String::as_str) {
        Some("all") => verify_all(&artifacts)?,
        Some("ratatui") => verify_ratatui(&artifacts, args.iter().any(|arg| arg == "--pty"))?,
        Some("native-wgpu") if args.iter().any(|arg| arg == "--app-window") => {
            verify_native_app_window(&artifacts)?
        }
        Some("native-wgpu") => verify_native_wgpu_headless(&artifacts)?,
        Some("browser-wgpu") => verify_browser_firefox(&artifacts)?,
        _ => bail!("expected verify target all|ratatui|native-wgpu|browser-wgpu"),
    };

    let report_path = artifacts.join("verify-report.json");
    fs::write(&report_path, serde_json::to_vec_pretty(&report)?)?;
    if report.results.iter().all(|result| result.passed)
        && args.first().map(String::as_str) == Some("all")
    {
        let success = json!({
            "git_commit": git_commit(&root).unwrap_or_else(|| "unknown".to_string()),
            "platform": env::consts::OS,
            "tool_versions": tool_versions(&root)?,
            "firefox_profile": root.join(".boon-local/firefox-profile"),
            "commands": ["bootstrap", "generate", "shaders", "cargo fmt --check", "cargo clippy --workspace -- -D warnings", "cargo test --workspace", "verify all"],
            "results": report.results,
            "timing_summaries": collect_timing_summaries(&artifacts)?,
        });
        fs::write(success_path, serde_json::to_vec_pretty(&success)?)?;
    } else if let Some(failed) = report.results.iter().find(|result| !result.passed) {
        bail!("{}", failed.message);
    }
    println!("wrote {}", report_path.display());
    Ok(())
}

fn quality_gates(root: &Path) -> Result<()> {
    run(Command::new("cargo")
        .current_dir(root)
        .args(["fmt", "--all", "--", "--check"]))?;
    run(Command::new("cargo").current_dir(root).args([
        "clippy",
        "--workspace",
        "--",
        "-D",
        "warnings",
    ]))?;
    run(Command::new("cargo")
        .current_dir(root)
        .args(["test", "--workspace"]))?;
    Ok(())
}

fn bench(args: &[String]) -> Result<()> {
    let example = args
        .first()
        .map(String::as_str)
        .context("usage: cargo xtask bench <todo_mvc|cells> --backend all")?;
    if !matches!(example, "todo_mvc" | "cells") {
        bail!("bench supports todo_mvc and cells hard gates, got `{example}`");
    }
    if args
        .windows(2)
        .any(|pair| pair[0] == "--backend" && pair[1] != "all")
    {
        bail!("bench currently supports `--backend all` for the hard gate");
    }
    let root = repo_root()?;
    let artifacts = root.join("target/boon-artifacts");
    fs::create_dir_all(&artifacts)?;
    bootstrap(false)?;
    generate()?;
    shaders()?;
    let report = verify_all(&artifacts)?;
    let failed = report.results.iter().find(|result| {
        !result.passed
            || (result.example != example
                && !(example == "todo_mvc" && result.example == "todo_mvc_physical"))
    });
    if let Some(failed) = failed.filter(|result| !result.passed) {
        bail!("{}", failed.message);
    }
    let bench_path = artifacts.join(format!("bench-{example}.json"));
    fs::write(
        &bench_path,
        serde_json::to_vec_pretty(&json!({
            "command": format!("bench {example} --backend all"),
            "example": example,
            "timing_summaries": collect_timing_summaries(&artifacts)?,
        }))?,
    )?;
    println!("wrote {}", bench_path.display());
    Ok(())
}

fn run_native(args: &[String]) -> Result<()> {
    let (example, hold_ms) = parse_run_args("native", args)?;
    let root = repo_root()?;
    let artifacts = root.join("target/boon-artifacts");
    fs::create_dir_all(&artifacts)?;
    bootstrap(false)?;
    generate()?;
    shaders()?;
    let result =
        run_native_app_window_example(example, &artifacts, Duration::from_millis(hold_ms))?;
    if !result.passed {
        bail!("{}", result.message);
    }
    println!("{}", serde_json::to_string_pretty(&result)?);
    println!("artifact_dir {}", result.artifact_dir.display());
    Ok(())
}

fn playground_native(args: &[String]) -> Result<()> {
    let (example, hold_ms) = parse_playground_args(args)?;
    bootstrap(false)?;
    generate()?;
    shaders()?;
    println!(
        "starting native playground with example `{example}`; press Esc in the window to quit"
    );
    run_native_playground(example, Duration::from_millis(hold_ms))
}

fn run_ratatui(args: &[String]) -> Result<()> {
    let (example, hold_ms) = parse_run_args("ratatui", args)?;
    let root = repo_root()?;
    let artifacts = root.join("target/boon-artifacts");
    fs::create_dir_all(&artifacts)?;
    generate()?;
    let report = verify_ratatui(&artifacts, false)?;
    let result = report
        .results
        .into_iter()
        .find(|result| result.example == example)
        .context("ratatui runner did not produce the requested example")?;
    if !result.passed {
        bail!("{}", result.message);
    }
    let frame = fs::read_to_string(result.artifact_dir.join("frames.txt"))?;
    println!("{frame}");
    if hold_ms > 0 {
        std::thread::sleep(Duration::from_millis(hold_ms));
    }
    println!("{}", serde_json::to_string_pretty(&result)?);
    println!("artifact_dir {}", result.artifact_dir.display());
    Ok(())
}

fn run_browser(args: &[String]) -> Result<()> {
    let (example, hold_ms) = parse_run_args("browser", args)?;
    let root = repo_root()?;
    let artifacts = root.join("target/boon-artifacts");
    fs::create_dir_all(&artifacts)?;
    bootstrap(false)?;
    generate()?;
    shaders()?;
    let report = verify_browser_firefox(&artifacts)?;
    let result = report
        .results
        .into_iter()
        .find(|result| result.example == example)
        .context("browser runner did not produce the requested example")?;
    if !result.passed {
        bail!("{}", result.message);
    }
    if hold_ms > 0 {
        std::thread::sleep(Duration::from_millis(hold_ms));
    }
    println!("{}", serde_json::to_string_pretty(&result)?);
    println!("artifact_dir {}", result.artifact_dir.display());
    Ok(())
}

fn parse_playground_args<'a>(args: &'a [String]) -> Result<(&'a str, u64)> {
    let example = args
        .windows(2)
        .find(|pair| pair[0] == "--example")
        .map(|pair| pair[1].as_str())
        .or_else(|| {
            args.first()
                .map(String::as_str)
                .filter(|arg| !arg.starts_with("--"))
        })
        .unwrap_or("todo_mvc");
    if !list_examples().contains(&example) {
        bail!("unknown example `{example}`; run `cargo xtask examples list`");
    }
    let hold_ms = args
        .windows(2)
        .find(|pair| pair[0] == "--hold-ms")
        .map(|pair| pair[1].parse::<u64>())
        .transpose()
        .context("--hold-ms must be an integer")?
        .unwrap_or(3_600_000);
    Ok((example, hold_ms))
}

fn parse_run_args<'a>(platform: &str, args: &'a [String]) -> Result<(&'a str, u64)> {
    let example = args
        .windows(2)
        .find(|pair| pair[0] == "--example")
        .map(|pair| pair[1].as_str())
        .or_else(|| args.first().map(String::as_str))
        .with_context(|| {
            format!("usage: cargo xtask run {platform} --example <name> [--hold-ms <ms>]")
        })?;
    if !list_examples().contains(&example) {
        bail!("unknown example `{example}`; run `cargo xtask examples list`");
    }
    let hold_ms = args
        .windows(2)
        .find(|pair| pair[0] == "--hold-ms")
        .map(|pair| pair[1].parse::<u64>())
        .transpose()
        .context("--hold-ms must be an integer")?
        .unwrap_or(3000);
    Ok((example, hold_ms))
}

fn repo_root() -> Result<PathBuf> {
    let mut dir = env::current_dir()?;
    loop {
        if dir.join("IMPLEMENTATION_PLAN.md").exists() {
            return Ok(dir);
        }
        if !dir.pop() {
            bail!("could not find repo root containing IMPLEMENTATION_PLAN.md");
        }
    }
}

fn command_exists(name: &str) -> bool {
    Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {name} >/dev/null 2>&1"))
        .status()
        .is_ok_and(|status| status.success())
}

fn repo_local_web_ext_version(tools: &Path) -> Result<Option<String>> {
    let package = tools.join("node_modules/web-ext/package.json");
    if !package.exists() {
        return Ok(None);
    }
    let value: serde_json::Value = serde_json::from_slice(&fs::read(&package)?)?;
    Ok(value
        .get("version")
        .and_then(|value| value.as_str())
        .map(str::to_string))
}

fn run(command: &mut Command) -> Result<()> {
    let status = command.status()?;
    if !status.success() {
        bail!("command failed with {status}: {command:?}");
    }
    Ok(())
}

fn command_stdout(command: &mut Command) -> Result<String> {
    let output = command.output()?;
    if !output.status.success() {
        bail!("command failed with {}: {command:?}", output.status);
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn tool_versions(root: &Path) -> Result<serde_json::Value> {
    let web_ext_version = repo_local_web_ext_version(&root.join(".boon-local/tools"))?;
    Ok(json!({
        "rustc": command_stdout(Command::new("rustc").arg("--version")).unwrap_or_else(|err| format!("unavailable: {err}")),
        "cargo": command_stdout(Command::new("cargo").arg("--version")).unwrap_or_else(|err| format!("unavailable: {err}")),
        "firefox": command_stdout(Command::new("firefox").arg("--version")).unwrap_or_else(|err| format!("unavailable: {err}")),
        "web_ext": web_ext_version.unwrap_or_else(|| "missing".to_string()),
    }))
}

fn collect_timing_summaries(artifacts: &Path) -> Result<Vec<serde_json::Value>> {
    let mut summaries = Vec::new();
    if !artifacts.exists() {
        return Ok(summaries);
    }
    for entry in WalkDir::new(artifacts).into_iter().filter_map(Result::ok) {
        if entry.file_type().is_file() && entry.file_name() == "timings.json" {
            let value: serde_json::Value = serde_json::from_slice(&fs::read(entry.path())?)?;
            summaries.push(json!({
                "path": entry.path(),
                "summary": value,
            }));
        }
    }
    summaries.sort_by(|a, b| {
        a.get("path")
            .and_then(|value| value.as_str())
            .cmp(&b.get("path").and_then(|value| value.as_str()))
    });
    Ok(summaries)
}

fn git_commit(root: &Path) -> Option<String> {
    let output = Command::new("git")
        .current_dir(root)
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}
