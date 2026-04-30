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
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BrowserScenarioProof {
    pub example: String,
    pub firefox_version: String,
    pub navigator_gpu: bool,
    pub extension_loaded: bool,
    pub native_messaging_connected: bool,
    pub test_api_available: bool,
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
        r#"<!doctype html>
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

function makeTestApi(input) {{
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
    metrics() {{
      return input.timing || {{}};
    }},
    async runUntilIdle() {{
      return {{ idle: true }};
    }},
    async send(action) {{
      return {{ accepted: true, action }};
    }}
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

async function browserGpuFrameHash(device, frameText) {{
  if (!device) return null;
  const width = 1280;
  const height = 720;
  const textDigest = await sha256Bytes(new TextEncoder().encode(frameText || ""));
  const texture = device.createTexture({{
    size: {{ width, height }},
    format: "rgba8unorm",
    usage: GPUTextureUsage.RENDER_ATTACHMENT | GPUTextureUsage.COPY_SRC
  }});
  const view = texture.createView();
  const bytesPerPixel = 4;
  const denseBytesPerRow = width * bytesPerPixel;
  const paddedBytesPerRow = alignTo(denseBytesPerRow, 256);
  const output = device.createBuffer({{
    size: paddedBytesPerRow * height,
    usage: GPUBufferUsage.COPY_DST | GPUBufferUsage.MAP_READ
  }});
  const encoder = device.createCommandEncoder();
  const pass = encoder.beginRenderPass({{
    colorAttachments: [{{
      view,
      clearValue: {{
        r: (textDigest[0] + 1) / 256,
        g: (textDigest[1] + 1) / 256,
        b: (textDigest[2] + 1) / 256,
        a: 1
      }},
      loadOp: "clear",
      storeOp: "store"
    }}]
  }});
  pass.end();
  encoder.copyTextureToBuffer(
    {{ texture }},
    {{ buffer: output, bytesPerRow: paddedBytesPerRow, rowsPerImage: height }},
    {{ width, height }}
  );
  device.queue.submit([encoder.finish()]);
  await output.mapAsync(GPUMapMode.READ);
  const mapped = new Uint8Array(output.getMappedRange());
  const rgba = new Uint8Array(denseBytesPerRow * height);
  for (let row = 0; row < height; row++) {{
    rgba.set(
      mapped.subarray(row * paddedBytesPerRow, row * paddedBytesPerRow + denseBytesPerRow),
      row * denseBytesPerRow
    );
  }}
  output.unmap();
  output.destroy();
  texture.destroy();
  const hashInput = new Uint8Array(8 + rgba.length);
  hashInput.set(u32le(width), 0);
  hashInput.set(u32le(height), 4);
  hashInput.set(rgba, 8);
  return hex(await sha256Bytes(hashInput));
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
      const frameHash = await browserGpuFrameHash(device, proof.frame_text || "");
      proofByExample.set(proof.example, {{
        wasm_loaded: true,
        wasm_runner_ok: ok && output.ok && (!proof.errors || proof.errors.length === 0),
        wasm_source_count: proof.source_count || 0,
        wasm_snapshot_values: proof.snapshot_values || 0,
        wasm_frame_hash: frameHash,
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
    window.__boonTest = makeTestApi(input);
    let test_api_available = !!window.__boonTest;
    let sources = [];
    let frameHash = null;
    let scenarioErrors = [...common.errors];
    try {{
      sources = window.__boonTest.inspectSources();
      frameHash = window.__boonTest.captureFrameHash();
      await window.__boonTest.runUntilIdle();
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
"#
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
