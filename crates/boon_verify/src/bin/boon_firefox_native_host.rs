use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::PathBuf;

fn main() -> Result<()> {
    let request = read_native_message().context("reading Firefox native message")?;
    append_log(&request).context("logging Firefox native message")?;
    let response = handle_message(request);
    write_native_message(&response).context("writing Firefox native response")?;
    Ok(())
}

fn handle_message(request: Value) -> Value {
    match request.get("type").and_then(Value::as_str) {
        Some("ping") => json!({
            "type": "pong",
            "host": "boon-firefox-native-host",
            "protocol": 1
        }),
        Some("doctor") => json!({
            "type": "doctor-result",
            "native_messaging_connected": true,
            "host": "boon-firefox-native-host"
        }),
        Some("browser-doctor-result") => json!({
            "type": "browser-doctor-ack",
            "native_messaging_connected": true,
            "host": "boon-firefox-native-host"
        }),
        Some("browser-scenario-result") => json!({
            "type": "browser-scenario-ack",
            "native_messaging_connected": true,
            "host": "boon-firefox-native-host"
        }),
        _ => json!({
            "type": "error",
            "message": "unknown native host message",
            "request": request
        }),
    }
}

fn read_native_message() -> Result<Value> {
    let mut len = [0u8; 4];
    std::io::stdin().read_exact(&mut len)?;
    let len = u32::from_le_bytes(len) as usize;
    let mut bytes = vec![0u8; len];
    std::io::stdin().read_exact(&mut bytes)?;
    Ok(serde_json::from_slice(&bytes)?)
}

fn write_native_message(value: &Value) -> Result<()> {
    let bytes = serde_json::to_vec(value)?;
    let len = u32::try_from(bytes.len()).context("native response too large")?;
    let mut stdout = std::io::stdout();
    stdout.write_all(&len.to_le_bytes())?;
    stdout.write_all(&bytes)?;
    stdout.flush()?;
    Ok(())
}

fn append_log(request: &Value) -> Result<()> {
    let root = repo_root_from_exe()?;
    let log_dir = root.join(".boon-local/firefox-native-host");
    fs::create_dir_all(&log_dir)?;
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_dir.join("messages.jsonl"))?;
    let line = json!({
        "request": request,
        "received_unix_ms": unix_ms()
    });
    writeln!(file, "{}", serde_json::to_string(&line)?)?;
    Ok(())
}

fn repo_root_from_exe() -> Result<PathBuf> {
    let exe = std::env::current_exe()?;
    let debug_dir = exe
        .parent()
        .context("native host executable has no parent")?;
    let target_dir = debug_dir
        .parent()
        .context("native host debug dir has no parent")?;
    let root = target_dir
        .parent()
        .context("native host target dir has no parent")?;
    Ok(root.to_path_buf())
}

fn unix_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0)
}
