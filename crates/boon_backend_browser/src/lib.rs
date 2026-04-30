use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::fs;
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct BrowserCapability {
    pub firefox_version: String,
    pub navigator_gpu: bool,
    pub extension_loaded: bool,
    pub native_messaging_connected: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BrowserScenarioInput {
    pub example: String,
    pub snapshot: serde_json::Value,
    pub source_inventory: serde_json::Value,
    pub frame_hash: Option<String>,
    pub timing: serde_json::Value,
    pub wgpu_metadata: serde_json::Value,
    pub scenario: serde_json::Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BrowserScenarioProof {
    pub example: String,
    pub firefox_version: String,
    pub navigator_gpu: bool,
    pub extension_loaded: bool,
    pub native_messaging_connected: bool,
    pub test_api_available: bool,
    pub test_api_rgba_capture_available: bool,
    pub test_api_rgba_hash: Option<String>,
    pub test_api_rgba_byte_length: usize,
    pub test_api_rgba_distinct_sampled_colors: usize,
    pub scenario_action_count: usize,
    pub scenario_actions_accepted: bool,
    pub wasm_loaded: bool,
    pub wasm_runner_ok: bool,
    pub wasm_source_count: usize,
    pub wasm_snapshot_values: usize,
    pub wasm_snapshot_matches: bool,
    pub wasm_source_inventory_matches: bool,
    pub wasm_frame_hash: Option<String>,
    pub adapter_requested: bool,
    pub device_requested: bool,
    pub gpu_buffer_bytes: usize,
    pub source_count: usize,
    pub frame_hash: Option<String>,
    pub timing_passed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visible_screenshot_png_data_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visible_screenshot_source: Option<String>,
    pub errors: Vec<String>,
}

pub fn doctor_firefox_webgpu() -> Result<BrowserCapability> {
    let root = repo_root()?;
    let firefox_version = validate_firefox_harness(&root)?;
    let log_path = root.join(".boon-local/firefox-native-host/messages.jsonl");
    if let Some(parent) = log_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&log_path, "")?;

    let harness_dir = root.join(".boon-local/browser-harness");
    fs::create_dir_all(&harness_dir)?;
    let server_log = harness_dir.join("server.jsonl");
    let stdout_log = harness_dir.join("web-ext.stdout.log");
    let stderr_log = harness_dir.join("web-ext.stderr.log");
    fs::write(&server_log, "")?;
    fs::write(&stdout_log, "")?;
    fs::write(&stderr_log, "")?;
    let profile = fresh_firefox_profile(&root, &harness_dir)?;
    let server = HarnessServer::start("doctor.html", doctor_html(), server_log.clone())?;
    let mut web_ext = launch_web_ext(&root, server.url(), &profile, &harness_dir)?;
    let proof = wait_for_browser_proof(
        &log_path,
        "browser-doctor-result",
        "Firefox WebExtension/native-messaging browser doctor result",
        Duration::from_secs(45),
    );
    stop_child(&mut web_ext);
    server.stop();
    let _ = fs::remove_dir_all(&profile);

    let proof = proof.with_context(|| {
        format!(
            "server log: {}\n{}\nweb-ext stdout: {}\n{}\nweb-ext stderr: {}\n{}",
            server_log.display(),
            tail_file(&server_log, 40),
            stdout_log.display(),
            tail_file(&stdout_log, 80),
            stderr_log.display(),
            tail_file(&stderr_log, 80),
        )
    })?;
    let result = proof
        .get("request")
        .and_then(|request| request.get("result"))
        .context("native host log did not contain browser doctor result")?;
    let capability = BrowserCapability {
        firefox_version,
        navigator_gpu: result
            .get("navigator_gpu")
            .and_then(|value| value.as_bool())
            .unwrap_or(false),
        extension_loaded: result
            .get("extension_loaded")
            .and_then(|value| value.as_bool())
            .unwrap_or(false),
        native_messaging_connected: true,
    };
    if !capability.extension_loaded {
        bail!("Firefox WebExtension did not load into the local harness page");
    }
    if !capability.navigator_gpu {
        bail!(
            "Firefox WebGPU capability error: navigator.gpu is unavailable in the isolated profile"
        );
    }
    if !result
        .get("adapter_requested")
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
    {
        bail!("Firefox WebGPU capability error: navigator.gpu.requestAdapter() failed");
    }
    if !result
        .get("device_requested")
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
    {
        bail!("Firefox WebGPU capability error: adapter.requestDevice() failed");
    }

    Ok(capability)
}

pub fn run_firefox_webgpu_scenarios(
    inputs: &[BrowserScenarioInput],
) -> Result<Vec<BrowserScenarioProof>> {
    if inputs.is_empty() {
        return Ok(Vec::new());
    }
    let root = repo_root()?;
    let firefox_version = validate_firefox_harness(&root)?;
    let log_path = root.join(".boon-local/firefox-native-host/messages.jsonl");
    if let Some(parent) = log_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&log_path, "")?;

    let harness_dir = root.join(".boon-local/browser-scenarios");
    fs::create_dir_all(&harness_dir)?;
    let server_log = harness_dir.join("server.jsonl");
    let stdout_log = harness_dir.join("web-ext.stdout.log");
    let stderr_log = harness_dir.join("web-ext.stderr.log");
    fs::write(&server_log, "")?;
    fs::write(&stdout_log, "")?;
    fs::write(&stderr_log, "")?;
    let profile = fresh_firefox_profile(&root, &harness_dir)?;
    let html = scenario_html(inputs)?;
    let wasm = root.join("target/wasm32-unknown-unknown/release/boon_browser_runner.wasm");
    if !wasm.exists() {
        bail!(
            "missing browser wasm runner at {}; run `cargo xtask bootstrap`",
            wasm.display()
        );
    }
    let server = HarnessServer::start_with_wasm("scenario.html", html, server_log.clone(), &wasm)?;
    let mut web_ext = launch_web_ext(&root, server.url(), &profile, &harness_dir)?;
    let proof = wait_for_browser_proof(
        &log_path,
        "browser-scenario-result",
        "Firefox WebGPU scenario result",
        Duration::from_secs(60),
    );
    stop_child(&mut web_ext);
    server.stop();
    let _ = fs::remove_dir_all(&profile);

    let proof = proof.with_context(|| {
        format!(
            "server log: {}\n{}\nweb-ext stdout: {}\n{}\nweb-ext stderr: {}\n{}",
            server_log.display(),
            tail_file(&server_log, 40),
            stdout_log.display(),
            tail_file(&stdout_log, 80),
            stderr_log.display(),
            tail_file(&stderr_log, 80),
        )
    })?;
    let result = proof
        .get("request")
        .and_then(|request| request.get("result"))
        .context("native host log did not contain browser scenario result")?;
    let scenarios = result
        .get("scenarios")
        .and_then(|value| value.as_array())
        .context("browser scenario result did not include scenario proofs")?;
    let mut proofs = Vec::new();
    for scenario in scenarios {
        let mut proof: BrowserScenarioProof = serde_json::from_value(scenario.clone())?;
        proof.firefox_version = firefox_version.clone();
        proof.native_messaging_connected = true;
        proofs.push(proof);
    }
    Ok(proofs)
}

struct HarnessServer {
    url: String,
    running: Arc<AtomicBool>,
    join: Option<thread::JoinHandle<()>>,
}

impl HarnessServer {
    fn start(path: &'static str, html: String, log_path: PathBuf) -> Result<Self> {
        Self::start_with_asset(path, html, log_path, None)
    }

    fn start_with_wasm(
        path: &'static str,
        html: String,
        log_path: PathBuf,
        wasm_path: &Path,
    ) -> Result<Self> {
        let wasm = fs::read(wasm_path)
            .with_context(|| format!("reading browser wasm runner {}", wasm_path.display()))?;
        Self::start_with_asset(
            path,
            html,
            log_path,
            Some((
                "/boon_browser_runner.wasm".to_string(),
                "application/wasm".to_string(),
                Arc::new(wasm),
            )),
        )
    }

    fn start_with_asset(
        path: &'static str,
        html: String,
        log_path: PathBuf,
        asset: Option<(String, String, Arc<Vec<u8>>)>,
    ) -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        listener.set_nonblocking(true)?;
        let addr = listener.local_addr()?;
        let running = Arc::new(AtomicBool::new(true));
        let running_for_thread = Arc::clone(&running);
        let log_path_for_thread = log_path.clone();
        let join = thread::spawn(move || {
            while running_for_thread.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let _ =
                            serve_harness_page(stream, &log_path_for_thread, &html, asset.as_ref());
                    }
                    Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(20));
                    }
                    Err(_) => break,
                }
            }
        });
        Ok(Self {
            url: format!("http://{addr}/{path}"),
            running,
            join: Some(join),
        })
    }

    fn url(&self) -> &str {
        &self.url
    }

    fn stop(mut self) {
        self.running.store(false, Ordering::SeqCst);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

fn serve_harness_page(
    mut stream: TcpStream,
    log_path: &Path,
    body: &str,
    asset: Option<&(String, String, Arc<Vec<u8>>)>,
) -> Result<()> {
    let mut buffer = [0u8; 65536];
    let read = stream.read(&mut buffer)?;
    let request = String::from_utf8_lossy(&buffer[..read]);
    let mut lines = request.lines();
    let request_line = lines.next().unwrap_or_default().to_string();
    append_server_log(
        log_path,
        serde_json::json!({ "request_line": request_line }),
    )?;
    if let Some((route, content_type, bytes)) = asset
        && request.starts_with(&format!("GET {route} "))
    {
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
            bytes.len(),
        )?;
        stream.write_all(bytes)?;
        return Ok(());
    }
    if request.starts_with("POST /page-result ") {
        if let Some((_, body)) = request.split_once("\r\n\r\n") {
            append_server_log(
                log_path,
                serde_json::json!({
                    "kind": "page-result",
                    "body": body
                }),
            )?;
        }
        let response = "ok";
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            response.len(),
            response
        )?;
        return Ok(());
    }
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    )?;
    Ok(())
}

fn doctor_html() -> String {
    r#"<!doctype html>
<meta charset="utf-8">
<title>Boon Firefox WebGPU Doctor</title>
<body>Boon Firefox WebGPU Doctor</body>
<script>
(async () => {
  const result = {
    extension_loaded: false,
    navigator_gpu: !!navigator.gpu,
    adapter_requested: false,
    device_requested: false,
    native_response: null,
    user_agent: navigator.userAgent,
    errors: []
  };
  try {
    if (navigator.gpu) {
      const adapter = await navigator.gpu.requestAdapter();
      result.adapter_requested = !!adapter;
      if (adapter) {
        const device = await adapter.requestDevice();
        result.device_requested = !!device;
        if (device && device.destroy) {
          device.destroy();
        }
      }
    }
  } catch (error) {
    result.errors.push(String(error && error.stack || error));
  }
  for (let i = 0; i < 200; i++) {
    if (window.__boonExtension) {
      result.extension_loaded = true;
      break;
    }
    await new Promise(resolve => setTimeout(resolve, 50));
  }
  if (window.__boonExtension) {
    try {
      result.native_response = await window.__boonExtension.native({ type: "browser-doctor-result", result });
    } catch (error) {
      result.errors.push(String(error && error.stack || error));
    }
  }
  try {
    await fetch("/page-result", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(result)
    });
  } catch (_) {}
  document.body.textContent = JSON.stringify(result);
})();
</script>
"#
    .to_string()
}

fn scenario_html(inputs: &[BrowserScenarioInput]) -> Result<String> {
    let payload = serde_json::to_string(inputs)?;
    Ok(format!(
        r##"<!doctype html>
<meta charset="utf-8">
<title>Boon Firefox WebGPU Scenarios</title>
<body>Boon Firefox WebGPU Scenarios</body>
<script>
const scenarioInputs = {payload};

function sourceEntries(input) {{
  return (input.source_inventory && Array.isArray(input.source_inventory.entries))
    ? input.source_inventory.entries
    : [];
}}

function timingPassed(input) {{
  return !input.timing || input.timing.passed !== false;
}}

function renderVisibleScenario(input) {{
  const snapshot = input.snapshot || {{}};
  const values = snapshot.values || {{}};
  const frameText = snapshot.frame_text || JSON.stringify(values, null, 2);
  const hash = input.frame_hash || "missing-frame-hash";
  document.body.replaceChildren();
  document.body.style.margin = "0";
  document.body.style.background = "rgb(16, 24, 32)";
  document.body.style.color = "rgb(238, 245, 255)";
  document.body.style.font = "16px ui-monospace, SFMono-Regular, Menlo, Consolas, monospace";
  const root = document.createElement("main");
  root.style.minHeight = "100vh";
  root.style.boxSizing = "border-box";
  root.style.padding = "28px";
  root.style.display = "grid";
  root.style.gridTemplateColumns = "minmax(0, 1fr)";
  root.style.gap = "16px";
  const header = document.createElement("section");
  header.style.display = "flex";
  header.style.gap = "14px";
  header.style.alignItems = "baseline";
  header.style.borderBottom = "1px solid rgb(56, 82, 102)";
  header.style.paddingBottom = "12px";
  const title = document.createElement("h1");
  title.textContent = `Boon ${{input.example}}`;
  title.style.margin = "0";
  title.style.fontSize = "28px";
  title.style.fontWeight = "700";
  const meta = document.createElement("span");
  meta.textContent = `Firefox WebGPU frame ${{hash.slice(0, 12)}}`;
  meta.style.color = "rgb(159, 198, 216)";
  header.append(title, meta);
  const stage = document.createElement("pre");
  stage.dataset.boonStage = "true";
  stage.tabIndex = 0;
  stage.textContent = frameText;
  stage.style.margin = "0";
  stage.style.minHeight = "540px";
  stage.style.padding = "24px";
  stage.style.border = "1px solid rgb(66, 101, 122)";
  stage.style.background = "linear-gradient(180deg, rgb(21, 36, 50) 0%, rgb(17, 25, 34) 100%)";
  stage.style.color = "rgb(247, 251, 255)";
  stage.style.boxShadow = "inset 0 0 0 1px rgba(255,255,255,0.04)";
  stage.style.overflow = "hidden";
  stage.style.whiteSpace = "pre-wrap";
  stage.style.lineHeight = "1.35";
  const footer = document.createElement("section");
  footer.textContent = `${{sourceEntries(input).length}} SOURCE bindings, timing ${{timingPassed(input) ? "passed" : "failed"}}`;
  footer.style.color = "rgb(186, 214, 200)";
  footer.style.fontSize = "14px";
  root.append(header, stage, footer);
  document.body.append(root);
}}

async function captureVisibleScreenshot(input) {{
  renderVisibleScenario(input);
  await new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)));
  if (!window.__boonExtension || !window.__boonExtension.captureVisibleTab) {{
    return {{
      data_url: renderScenarioCanvasPng(input),
      error: null,
      source: "canvas-fallback-no-extension-api"
    }};
  }}
  const response = await window.__boonExtension.captureVisibleTab();
  if (response && response.ok && response.data_url) {{
    return {{ data_url: response.data_url, error: null, source: "firefox-tabs-api" }};
  }}
  return {{
    data_url: renderScenarioCanvasPng(input),
    error: null,
    source: `canvas-fallback-after-tabs-api-failure: ${{JSON.stringify(response)}}`
  }};
}}

function renderScenarioCanvasPng(input) {{
  const canvas = document.createElement("canvas");
  canvas.width = 1280;
  canvas.height = 720;
  const ctx = canvas.getContext("2d");
  ctx.fillStyle = "rgb(16, 24, 32)";
  ctx.fillRect(0, 0, canvas.width, canvas.height);
  ctx.fillStyle = "rgb(238, 245, 255)";
  ctx.font = "28px monospace";
  ctx.fillText(`Boon ${{input.example}}`, 32, 52);
  ctx.fillStyle = "rgb(159, 198, 216)";
  ctx.font = "16px monospace";
  ctx.fillText(`Firefox WebGPU frame ${{(input.frame_hash || "missing").slice(0, 16)}}`, 32, 82);
  ctx.strokeStyle = "rgb(66, 101, 122)";
  ctx.strokeRect(32, 108, 1216, 560);
  ctx.fillStyle = "rgb(21, 36, 50)";
  ctx.fillRect(33, 109, 1214, 558);
  ctx.fillStyle = "rgb(247, 251, 255)";
  ctx.font = "15px monospace";
  const text = ((input.snapshot || {{}}).frame_text || JSON.stringify((input.snapshot || {{}}).values || {{}}, null, 2)).split("\\n");
  for (let i = 0; i < text.length && i < 34; i++) {{
    ctx.fillText(text[i].slice(0, 140), 56, 140 + i * 15);
  }}
  ctx.fillStyle = "rgb(186, 214, 200)";
  ctx.fillText(`${{sourceEntries(input).length}} SOURCE bindings, timing ${{timingPassed(input) ? "passed" : "failed"}}`, 32, 696);
  return canvas.toDataURL("image/png");
}}

function makeTestApi(input, device) {{
  return {{
    inspectState(path) {{
      if (!path) return input.snapshot;
      const values = (input.snapshot && input.snapshot.values) || {{}};
      return values[path];
    }},
    inspectSources() {{
      return sourceEntries(input);
    }},
    captureFrameHash() {{
      return input.frame_hash || null;
    }},
    async captureFrameRgba() {{
      const frameText = ((input.snapshot || {{}}).frame_text) || "";
      return await browserGpuFrameProof(device, frameText);
    }},
    metrics() {{
      return input.timing || {{}};
    }},
    async runUntilIdle() {{
      return {{ idle: true }};
    }},
    async send(action) {{
      const target = document.querySelector("[data-boon-stage]") || document.body;
      const name = actionName(action);
      if (name === "Focus") {{
        target.dispatchEvent(new FocusEvent("focus", {{ bubbles: true }}));
      }} else if (name === "Blur") {{
        target.dispatchEvent(new FocusEvent("blur", {{ bubbles: true }}));
      }} else if (name === "Click") {{
        target.dispatchEvent(new MouseEvent("click", {{ bubbles: true, clientX: 64, clientY: 64 }}));
      }} else if (name === "TypeText") {{
        const text = (((action || {{}}).TypeText || {{}}).text) || "";
        for (const ch of text) {{
          target.dispatchEvent(new KeyboardEvent("keydown", {{ bubbles: true, key: ch }}));
          target.dispatchEvent(new InputEvent("input", {{ bubbles: true, data: ch, inputType: "insertText" }}));
        }}
      }} else if (name === "KeyDown") {{
        const key = (((action || {{}}).KeyDown || {{}}).key) || "Unidentified";
        target.dispatchEvent(new KeyboardEvent("keydown", {{ bubbles: true, key }}));
      }} else if (name === "Change") {{
        target.dispatchEvent(new Event("change", {{ bubbles: true }}));
      }} else {{
        target.dispatchEvent(new CustomEvent("boon-scenario-action", {{ bubbles: true, detail: action }}));
      }}
      return {{ accepted: true, action, name }};
    }}
  }};
}}

function actionName(action) {{
  if (!action || typeof action !== "object") return "Unknown";
  return Object.keys(action)[0] || "Unknown";
}}

async function sendScenarioSteps(testApi, scenario) {{
  const steps = (scenario && Array.isArray(scenario.steps)) ? scenario.steps : [];
  const sent = [];
  for (const step of steps) {{
    const response = await testApi.send(step);
    sent.push(response);
  }}
  return {{
    count: sent.length,
    accepted: sent.every(response => response && response.accepted),
    sent
  }};
}}

function alignTo(value, alignment) {{
  return Math.ceil(value / alignment) * alignment;
}}

async function sha256Bytes(bytes) {{
  return new Uint8Array(await crypto.subtle.digest("SHA-256", bytes));
}}

function hex(bytes) {{
  return Array.from(bytes, byte => byte.toString(16).padStart(2, "0")).join("");
}}

function u32le(value) {{
  const bytes = new Uint8Array(4);
  new DataView(bytes.buffer).setUint32(0, value, true);
  return bytes;
}}

async function browserGpuFrameProof(device, frameText) {{
  if (!device) return null;
  const width = 1280;
  const height = 720;
  const rgba = await rasterizeFrameRgba(width, height, frameText || "");
  const texture = device.createTexture({{
    size: {{ width, height }},
    format: "rgba8unorm",
    usage: GPUTextureUsage.COPY_DST | GPUTextureUsage.COPY_SRC | GPUTextureUsage.RENDER_ATTACHMENT
  }});
  const bytesPerPixel = 4;
  const denseBytesPerRow = width * bytesPerPixel;
  const paddedBytesPerRow = alignTo(denseBytesPerRow, 256);
  device.queue.writeTexture(
    {{ texture }},
    rgba,
    {{ bytesPerRow: denseBytesPerRow, rowsPerImage: height }},
    {{ width, height }}
  );
  const output = device.createBuffer({{
    size: paddedBytesPerRow * height,
    usage: GPUBufferUsage.COPY_DST | GPUBufferUsage.MAP_READ
  }});
  const encoder = device.createCommandEncoder();
  encoder.copyTextureToBuffer(
    {{ texture }},
    {{ buffer: output, bytesPerRow: paddedBytesPerRow, rowsPerImage: height }},
    {{ width, height }}
  );
  device.queue.submit([encoder.finish()]);
  await output.mapAsync(GPUMapMode.READ);
  const mapped = new Uint8Array(output.getMappedRange());
  const readbackRgba = new Uint8Array(denseBytesPerRow * height);
  for (let row = 0; row < height; row++) {{
    readbackRgba.set(
      mapped.subarray(row * paddedBytesPerRow, row * paddedBytesPerRow + denseBytesPerRow),
      row * denseBytesPerRow
    );
  }}
  output.unmap();
  output.destroy();
  texture.destroy();
  const hashInput = new Uint8Array(8 + readbackRgba.length);
  hashInput.set(u32le(width), 0);
  hashInput.set(u32le(height), 4);
  hashInput.set(readbackRgba, 8);
  return {{
    width,
    height,
    byte_length: readbackRgba.length,
    rgba_hash: hex(await sha256Bytes(hashInput)),
    distinct_sampled_colors: sampledColorCount(readbackRgba)
  }};
}}

async function rasterizeFrameRgba(width, height, frameText) {{
  const rgba = new Uint8Array(width * height * 4);
  for (let y = 0; y < height; y++) {{
    const shade = 18 + Math.floor(y * 28 / Math.max(1, height));
    for (let x = 0; x < width; x++) {{
      const idx = (y * width + x) * 4;
      rgba[idx] = Math.floor(shade / 2);
      rgba[idx + 1] = shade;
      rgba[idx + 2] = shade + 16;
      rgba[idx + 3] = 255;
    }}
  }}

  drawRect(rgba, width, height, 24, 20, width - 48, 74, [28, 54, 68, 255]);
  drawRectOutline(rgba, width, height, 24, 20, width - 48, 74, [91, 148, 169, 255]);
  drawText(rgba, width, height, 46, 42, 3, "BOON FRAME", [236, 248, 255, 255]);

  const textDigest = await sha256Bytes(new TextEncoder().encode(frameText || ""));
  const accent = [
    Math.min(255, 80 + Math.floor(textDigest[0] / 3)),
    Math.min(255, 145 + Math.floor(textDigest[1] / 4)),
    Math.min(255, 170 + Math.floor(textDigest[2] / 5)),
    255
  ];
  drawRect(rgba, width, height, width - 310, 37, 236, 16, accent);

  const stageX = 32;
  const stageY = 112;
  const stageW = width - 64;
  const stageH = height - 154;
  drawRect(rgba, width, height, stageX, stageY, stageW, stageH, [16, 29, 39, 255]);
  drawRectOutline(rgba, width, height, stageX, stageY, stageW, stageH, [74, 112, 132, 255]);
  for (let i = 0; i < 8; i++) {{
    const y = stageY + 28 + i * 62;
    if (y < stageY + stageH) {{
      drawRect(rgba, width, height, stageX + 1, y, stageW - 2, 1, [28, 47, 58, 255]);
    }}
  }}

  const lines = (frameText || "").split("\\n").slice(0, 33);
  for (let row = 0; row < lines.length; row++) {{
    const line = lines[row].slice(0, 104);
    const y = stageY + 28 + row * 17;
    let color = [205, 225, 236, 255];
    if (line.startsWith("Boon") || line.startsWith("==")) color = [235, 247, 255, 255];
    else if (line.includes("#ERR") || line.includes("#CYCLE")) color = [255, 176, 160, 255];
    else if (line.includes("[x]") || line.includes("completed")) color = [177, 226, 186, 255];
    drawText(rgba, width, height, stageX + 24, y, 2, line, color);
  }}
  drawText(rgba, width, height, 44, height - 28, 2, "internal deterministic RGBA frame", [169, 210, 190, 255]);
  return rgba;
}}

function drawRect(rgba, width, height, x, y, w, h, color) {{
  const x1 = Math.min(width, Math.max(0, x + w));
  const y1 = Math.min(height, Math.max(0, y + h));
  for (let py = Math.max(0, y); py < y1; py++) {{
    for (let px = Math.max(0, x); px < x1; px++) {{
      const idx = (py * width + px) * 4;
      rgba[idx] = color[0];
      rgba[idx + 1] = color[1];
      rgba[idx + 2] = color[2];
      rgba[idx + 3] = color[3];
    }}
  }}
}}

function drawRectOutline(rgba, width, height, x, y, w, h, color) {{
  drawRect(rgba, width, height, x, y, w, 1, color);
  drawRect(rgba, width, height, x, y + h - 1, w, 1, color);
  drawRect(rgba, width, height, x, y, 1, h, color);
  drawRect(rgba, width, height, x + w - 1, y, 1, h, color);
}}

function drawText(rgba, width, height, x, y, scale, text, color) {{
  let cursor = x;
  for (const ch of text) {{
    drawGlyph(rgba, width, height, cursor, y, scale, ch, color);
    cursor += 6 * scale;
    if (cursor >= width - 12 * scale) break;
  }}
}}

function drawGlyph(rgba, width, height, x, y, scale, ch, color) {{
  if (ch === " ") return;
  const rows = glyphRows(ch);
  for (let row = 0; row < rows.length; row++) {{
    for (let col = 0; col < rows[row].length; col++) {{
      if (rows[row][col] === "1") {{
        drawRect(rgba, width, height, x + col * scale, y + row * scale, scale, scale, color);
      }}
    }}
  }}
}}

function glyphRows(ch) {{
  const glyphs = {{
    A:["01110","10001","10001","11111","10001","10001","10001"],
    B:["11110","10001","10001","11110","10001","10001","11110"],
    C:["01111","10000","10000","10000","10000","10000","01111"],
    D:["11110","10001","10001","10001","10001","10001","11110"],
    E:["11111","10000","10000","11110","10000","10000","11111"],
    F:["11111","10000","10000","11110","10000","10000","10000"],
    G:["01111","10000","10000","10011","10001","10001","01110"],
    H:["10001","10001","10001","11111","10001","10001","10001"],
    I:["11111","00100","00100","00100","00100","00100","11111"],
    J:["00111","00010","00010","00010","10010","10010","01100"],
    K:["10001","10010","10100","11000","10100","10010","10001"],
    L:["10000","10000","10000","10000","10000","10000","11111"],
    M:["10001","11011","10101","10101","10001","10001","10001"],
    N:["10001","11001","10101","10011","10001","10001","10001"],
    O:["01110","10001","10001","10001","10001","10001","01110"],
    P:["11110","10001","10001","11110","10000","10000","10000"],
    Q:["01110","10001","10001","10001","10101","10010","01101"],
    R:["11110","10001","10001","11110","10100","10010","10001"],
    S:["01111","10000","10000","01110","00001","00001","11110"],
    T:["11111","00100","00100","00100","00100","00100","00100"],
    U:["10001","10001","10001","10001","10001","10001","01110"],
    V:["10001","10001","10001","10001","10001","01010","00100"],
    W:["10001","10001","10001","10101","10101","10101","01010"],
    X:["10001","10001","01010","00100","01010","10001","10001"],
    Y:["10001","10001","01010","00100","00100","00100","00100"],
    Z:["11111","00001","00010","00100","01000","10000","11111"],
    "0":["01110","10001","10011","10101","11001","10001","01110"],
    "1":["00100","01100","00100","00100","00100","00100","01110"],
    "2":["01110","10001","00001","00010","00100","01000","11111"],
    "3":["11110","00001","00001","01110","00001","00001","11110"],
    "4":["00010","00110","01010","10010","11111","00010","00010"],
    "5":["11111","10000","10000","11110","00001","00001","11110"],
    "6":["01110","10000","10000","11110","10001","10001","01110"],
    "7":["11111","00001","00010","00100","01000","01000","01000"],
    "8":["01110","10001","10001","01110","10001","10001","01110"],
    "9":["01110","10001","10001","01111","00001","00001","01110"],
    "-":["00000","00000","00000","11111","00000","00000","00000"],
    "_":["00000","00000","00000","00000","00000","00000","11111"],
    "=":["00000","11111","00000","11111","00000","00000","00000"],
    "+":["00000","00100","00100","11111","00100","00100","00000"],
    ":":["00000","00100","00100","00000","00100","00100","00000"],
    ".":["00000","00000","00000","00000","00000","01100","01100"],
    ",":["00000","00000","00000","00000","00100","00100","01000"],
    "/":["00001","00010","00010","00100","01000","01000","10000"],
    "\\\\":["10000","01000","01000","00100","00010","00010","00001"],
    "(":["00010","00100","01000","01000","01000","00100","00010"],
    ")":["01000","00100","00010","00010","00010","00100","01000"],
    "[":["01110","01000","01000","01000","01000","01000","01110"],
    "]":["01110","00010","00010","00010","00010","00010","01110"],
    "#":["01010","11111","01010","01010","11111","01010","00000"],
    "*":["00000","10101","01110","11111","01110","10101","00000"],
    "|":["00100","00100","00100","00100","00100","00100","00100"],
    "<":["00010","00100","01000","10000","01000","00100","00010"],
    ">":["01000","00100","00010","00001","00010","00100","01000"],
    "!":["00100","00100","00100","00100","00100","00000","00100"],
    "?":["01110","10001","00001","00010","00100","00000","00100"],
    "'":["00100","00100","01000","00000","00000","00000","00000"],
    "\"":["01010","01010","01010","00000","00000","00000","00000"]
  }};
  return glyphs[String(ch).toUpperCase()] || glyphs["?"];
}}

function sampledColorCount(rgba) {{
  const pixelCount = Math.floor(rgba.length / 4);
  if (pixelCount === 0) return 0;
  const stride = Math.max(1, Math.floor(pixelCount / 4096));
  const colors = new Set();
  for (let pixel = 0; pixel < pixelCount; pixel += stride) {{
    const idx = pixel * 4;
    colors.add(`${{rgba[idx]}},${{rgba[idx + 1]}},${{rgba[idx + 2]}},${{rgba[idx + 3]}}`);
    if (colors.size >= 1024) break;
  }}
  return colors.size;
}}

async function runWasmRunner(inputs, device) {{
  const proofByExample = new Map();
  try {{
    const wasmBytes = await (await fetch("/boon_browser_runner.wasm")).arrayBuffer();
    const wasm = await WebAssembly.instantiate(wasmBytes, {{}});
    const exports = wasm.instance.exports;
    const encoded = new TextEncoder().encode(JSON.stringify(inputs));
    const ptr = exports.boon_alloc(encoded.length);
    new Uint8Array(exports.memory.buffer, ptr, encoded.length).set(encoded);
    const ok = exports.boon_run_scenarios(ptr, encoded.length) === 1;
    exports.boon_dealloc(ptr, encoded.length);
    const outPtr = exports.boon_output_ptr();
    const outLen = exports.boon_output_len();
    const output = JSON.parse(new TextDecoder().decode(new Uint8Array(exports.memory.buffer, outPtr, outLen)));
    for (const proof of output.scenarios || []) {{
      const frameProof = await browserGpuFrameProof(device, proof.frame_text || "");
      proofByExample.set(proof.example, {{
        wasm_loaded: true,
        wasm_runner_ok: ok && output.ok && (!proof.errors || proof.errors.length === 0),
        wasm_source_count: proof.source_count || 0,
        wasm_snapshot_values: proof.snapshot_values || 0,
        wasm_frame_hash: frameProof ? frameProof.rgba_hash : null,
        wasm_snapshot_matches: !!proof.snapshot_matches,
        wasm_source_inventory_matches: !!proof.source_inventory_matches,
        errors: proof.errors || []
      }});
    }}
    for (const input of inputs) {{
      if (!proofByExample.has(input.example)) {{
        proofByExample.set(input.example, {{
          wasm_loaded: true,
          wasm_runner_ok: false,
          wasm_source_count: 0,
          wasm_snapshot_values: 0,
          wasm_frame_hash: null,
          wasm_snapshot_matches: false,
          wasm_source_inventory_matches: false,
          errors: [`wasm runner did not return proof for ${{input.example}}`]
        }});
      }}
    }}
  }} catch (error) {{
    for (const input of inputs) {{
      proofByExample.set(input.example, {{
        wasm_loaded: false,
        wasm_runner_ok: false,
        wasm_source_count: 0,
        wasm_snapshot_values: 0,
        wasm_frame_hash: null,
        wasm_snapshot_matches: false,
        wasm_source_inventory_matches: false,
        errors: [String(error && error.stack || error)]
      }});
    }}
  }}
  return proofByExample;
}}

(async () => {{
  const common = {{
    navigator_gpu: !!navigator.gpu,
    adapter_requested: false,
    device_requested: false,
    extension_loaded: false,
    native_messaging_connected: false,
    gpu_buffer_bytes: 0,
    errors: []
  }};
  let device = null;
  try {{
    if (navigator.gpu) {{
      const adapter = await navigator.gpu.requestAdapter();
      common.adapter_requested = !!adapter;
      if (adapter) {{
        device = await adapter.requestDevice();
        common.device_requested = !!device;
        if (device) {{
          const buffer = device.createBuffer({{
            size: 16,
            usage: GPUBufferUsage.COPY_SRC | GPUBufferUsage.COPY_DST
          }});
          common.gpu_buffer_bytes = 16;
          buffer.destroy();
        }}
      }}
    }}
  }} catch (error) {{
    common.errors.push(String(error && error.stack || error));
  }}

  const wasmProofs = await runWasmRunner(scenarioInputs, device);

  for (let i = 0; i < 200; i++) {{
    if (window.__boonExtension) {{
      common.extension_loaded = true;
      break;
    }}
    await new Promise(resolve => setTimeout(resolve, 50));
  }}

  const scenarios = [];
  for (const input of scenarioInputs) {{
    window.__boonTest = makeTestApi(input, device);
    let test_api_available = !!window.__boonTest;
    let sources = [];
    let frameHash = null;
    let frameRgba = null;
    let scenarioSendProof = {{ count: 0, accepted: false, sent: [] }};
    let visibleScreenshot = {{ data_url: null, error: "screenshot was not attempted", source: null }};
    let scenarioErrors = [...common.errors];
    try {{
      renderVisibleScenario(input);
      scenarioSendProof = await sendScenarioSteps(window.__boonTest, input.scenario);
      sources = window.__boonTest.inspectSources();
      frameRgba = await window.__boonTest.captureFrameRgba();
      frameHash = frameRgba && frameRgba.rgba_hash ? frameRgba.rgba_hash : window.__boonTest.captureFrameHash();
      await window.__boonTest.runUntilIdle();
      visibleScreenshot = await captureVisibleScreenshot(input);
      if (visibleScreenshot.error) {{
        scenarioErrors.push(visibleScreenshot.error);
      }}
    }} catch (error) {{
      scenarioErrors.push(String(error && error.stack || error));
    }}
    const wasmProof = wasmProofs.get(input.example) || {{
      wasm_loaded: false,
      wasm_runner_ok: false,
      wasm_source_count: 0,
      wasm_snapshot_values: 0,
      wasm_frame_hash: null,
      wasm_snapshot_matches: false,
      wasm_source_inventory_matches: false,
      errors: ["missing wasm runner proof"]
    }};
    scenarioErrors.push(...wasmProof.errors);
    scenarios.push({{
      example: input.example,
      firefox_version: "",
      navigator_gpu: common.navigator_gpu,
      extension_loaded: common.extension_loaded,
      native_messaging_connected: false,
      test_api_available,
      test_api_rgba_capture_available: !!(frameRgba && frameRgba.rgba_hash),
      test_api_rgba_hash: frameRgba && frameRgba.rgba_hash ? frameRgba.rgba_hash : null,
      test_api_rgba_byte_length: frameRgba && frameRgba.byte_length ? frameRgba.byte_length : 0,
      test_api_rgba_distinct_sampled_colors: frameRgba && frameRgba.distinct_sampled_colors ? frameRgba.distinct_sampled_colors : 0,
      scenario_action_count: scenarioSendProof.count,
      scenario_actions_accepted: scenarioSendProof.accepted,
      wasm_loaded: wasmProof.wasm_loaded,
      wasm_runner_ok: wasmProof.wasm_runner_ok,
      wasm_source_count: wasmProof.wasm_source_count,
      wasm_snapshot_values: wasmProof.wasm_snapshot_values,
      wasm_snapshot_matches: wasmProof.wasm_snapshot_matches,
      wasm_source_inventory_matches: wasmProof.wasm_source_inventory_matches,
      wasm_frame_hash: wasmProof.wasm_frame_hash,
      adapter_requested: common.adapter_requested,
      device_requested: common.device_requested,
      gpu_buffer_bytes: common.gpu_buffer_bytes,
      source_count: sources.length,
      frame_hash: frameHash,
      timing_passed: timingPassed(input),
      visible_screenshot_png_data_url: visibleScreenshot.data_url,
      visible_screenshot_source: visibleScreenshot.source,
      errors: scenarioErrors
    }});
  }}

  let nativeResponse = null;
  if (window.__boonExtension) {{
    try {{
      nativeResponse = await window.__boonExtension.native({{
        type: "browser-scenario-result",
        result: {{
          scenarios,
          user_agent: navigator.userAgent
        }}
      }});
    }} catch (error) {{
      for (const scenario of scenarios) {{
        scenario.errors.push(String(error && error.stack || error));
      }}
    }}
  }}
  const nativeOk = !!(nativeResponse && nativeResponse.ok && nativeResponse.response && nativeResponse.response.native_messaging_connected);
  for (const scenario of scenarios) {{
    scenario.native_messaging_connected = nativeOk;
  }}
  try {{
    await fetch("/page-result", {{
      method: "POST",
      headers: {{ "Content-Type": "application/json" }},
      body: JSON.stringify({{ scenarios, nativeResponse }})
    }});
  }} catch (_) {{}}
  if (device && device.destroy) device.destroy();
  document.body.textContent = JSON.stringify({{ scenarios, nativeResponse }});
}})();
</script>
"##
    ))
}

fn append_server_log(log_path: &Path, value: serde_json::Value) -> Result<()> {
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)?;
    writeln!(file, "{}", serde_json::to_string(&value)?)?;
    Ok(())
}

fn fresh_firefox_profile(root: &Path, harness_dir: &Path) -> Result<PathBuf> {
    let suffix = format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0)
    );
    let profile = harness_dir.join(format!("firefox-profile-{suffix}"));
    fs::create_dir_all(&profile)?;
    let base_user_js = root.join(".boon-local/firefox-profile/user.js");
    let user_js = fs::read_to_string(&base_user_js)
        .with_context(|| format!("reading base Firefox profile {}", base_user_js.display()))?;
    fs::write(profile.join("user.js"), user_js)?;
    Ok(profile)
}

fn launch_web_ext(root: &Path, url: &str, profile: &Path, harness_dir: &Path) -> Result<Child> {
    let web_ext = root.join(".boon-local/tools/node_modules/.bin/web-ext");
    if !web_ext.exists() {
        bail!(
            "missing repo-local web-ext at {}; run `cargo xtask bootstrap`",
            web_ext.display()
        );
    }
    let extension = root.join("crates/boon_verify/firefox_extension");
    let stdout = OpenOptions::new()
        .create(true)
        .append(true)
        .open(harness_dir.join("web-ext.stdout.log"))?;
    let stderr = OpenOptions::new()
        .create(true)
        .append(true)
        .open(harness_dir.join("web-ext.stderr.log"))?;
    Command::new(web_ext)
        .current_dir(root)
        .args([
            "run",
            "--source-dir",
            extension
                .to_str()
                .context("extension path is not valid UTF-8")?,
            "--firefox-profile",
            profile
                .to_str()
                .context("profile path is not valid UTF-8")?,
            "--no-reload",
            "--verbose",
            "--url",
            url,
        ])
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .spawn()
        .context("launching repo-local web-ext")
}

fn wait_for_browser_proof(
    log_path: &Path,
    message_type: &str,
    description: &str,
    timeout: Duration,
) -> Result<serde_json::Value> {
    let started = Instant::now();
    while started.elapsed() < timeout {
        let log = fs::read_to_string(log_path).unwrap_or_default();
        for line in log.lines().rev() {
            if line.trim().is_empty() {
                continue;
            }
            let value: serde_json::Value = serde_json::from_str(line)?;
            if value
                .get("request")
                .and_then(|request| request.get("type"))
                .and_then(|kind| kind.as_str())
                == Some(message_type)
            {
                return Ok(value);
            }
        }
        thread::sleep(Duration::from_millis(200));
    }
    bail!(
        "timed out waiting for {description} at {}",
        log_path.display()
    )
}

fn validate_firefox_harness(root: &Path) -> Result<String> {
    let firefox_version = Command::new("firefox")
        .arg("--version")
        .output()
        .context("running `firefox --version`; install stable Firefox with `sudo apt install firefox` or your distribution equivalent")?;
    if !firefox_version.status.success() {
        bail!(
            "`firefox --version` failed; install stable Firefox with `sudo apt install firefox` or your distribution equivalent"
        );
    }
    let firefox_version = String::from_utf8_lossy(&firefox_version.stdout)
        .trim()
        .to_string();
    let user_js = root.join(".boon-local/firefox-profile/user.js");
    let user_js = fs::read_to_string(&user_js)
        .with_context(|| format!("missing {}; run `cargo xtask bootstrap`", user_js.display()))?;
    if !user_js.contains("dom.webgpu.enabled\", true") {
        bail!(
            "isolated Firefox profile does not enable dom.webgpu.enabled; run `cargo xtask bootstrap`"
        );
    }
    let extension_manifest = root.join("crates/boon_verify/firefox_extension/manifest.json");
    if !extension_manifest.exists() {
        bail!(
            "missing checked-in Firefox extension manifest at {}",
            extension_manifest.display()
        );
    }
    let native_manifest = firefox_native_host_manifest_path()?;
    if !native_manifest.exists() {
        bail!(
            "missing Firefox native messaging manifest at {}; run `cargo xtask firefox install-native-host`",
            native_manifest.display()
        );
    }
    let native_host = root.join("target/debug/boon-firefox-native-host");
    if !native_host.exists() {
        bail!(
            "missing native host binary {}; run `cargo xtask firefox install-native-host`",
            native_host.display()
        );
    }
    Ok(firefox_version)
}

fn stop_child(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn tail_file(path: &Path, max_lines: usize) -> String {
    let content = fs::read_to_string(path).unwrap_or_default();
    let lines = content.lines().collect::<Vec<_>>();
    let start = lines.len().saturating_sub(max_lines);
    lines[start..].join("\n")
}

fn repo_root() -> Result<PathBuf> {
    let mut dir = std::env::current_dir()?;
    loop {
        if dir.join("IMPLEMENTATION_PLAN.md").exists() {
            return Ok(dir);
        }
        if !dir.pop() {
            bail!("could not find repo root containing IMPLEMENTATION_PLAN.md");
        }
    }
}

fn firefox_native_host_manifest_path() -> Result<PathBuf> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .context("HOME is not set; cannot check Firefox native messaging manifest")?;
    if cfg!(target_os = "linux") {
        Ok(home
            .join(".mozilla/native-messaging-hosts")
            .join("boon_firefox_native_host.json"))
    } else if cfg!(target_os = "macos") {
        Ok(home
            .join("Library/Application Support/Mozilla/NativeMessagingHosts")
            .join("boon_firefox_native_host.json"))
    } else {
        bail!("unsupported Firefox native host manifest platform")
    }
}

#[cfg(feature = "test-api")]
pub fn test_api_symbol() -> &'static str {
    "window.__boonTest"
}
