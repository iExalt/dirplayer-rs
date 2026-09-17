#!/usr/bin/env node
import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import { createServer } from "node:http";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { createRequire } from "node:module";

import { testSoundPlayback } from "./test-sound-playback.mjs";

const scriptDir = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(scriptDir, "..");
const childhoodRoot = process.env.CHILDHOOD_REDUX_ROOT || path.resolve(repoRoot, "../childhood-redux");
const runnerRoot = path.join(repoRoot, "vm-rust", "target", "browser_runner");
const fixtureRoot = path.join(childhoodRoot, "resources", "spybot");
const fixtureMovie = path.join(fixtureRoot, "spybot-nightfall-incident.dcr");
const sourceRevision = process.env.NATIVE_BEVY_SOURCE_REVISION;
const runStamp = new Date().toISOString().replaceAll(/[-:.TZ]/g, "");
const outputPath = process.env.NATIVE_BEVY_AUDIO_BASELINE || path.join(repoRoot, "test-results", `native-bevy-audio-baseline-${runStamp}.json`);

function sha256(filePath) {
  return createHash("sha256").update(fs.readFileSync(filePath)).digest("hex");
}

function firstMatchingFile(prefix, suffix) {
  const candidates = fs.readdirSync(runnerRoot).filter(
    (entry) => entry.startsWith(prefix) && entry.endsWith(suffix),
  );
  if (candidates.length !== 1) {
    throw new Error(`Expected one ${prefix}*${suffix} under ${runnerRoot}; found ${candidates.join(", ") || "none"}`);
  }
  return candidates[0];
}

const wasmJs = firstMatchingFile("mod-", ".js");
const wasmBinary = firstMatchingFile("mod-", "_bg.wasm");
if (wasmJs.replace(/\.js$/, "") !== wasmBinary.replace(/_bg\.wasm$/, "")) {
  throw new Error(`WASM JS/binary stems differ: ${wasmJs} vs ${wasmBinary}`);
}
const wasmJsPath = path.join(runnerRoot, wasmJs);
const wasmBinaryPath = path.join(runnerRoot, wasmBinary);
const apiStub = path.join(runnerRoot, "dirplayer-js-api.js");
const apiReal = path.join(runnerRoot, "dirplayer-js-api-real.js");
const ruffleRuntime = path.join(repoRoot, "public", "ruffle");
const requireFromRepo = createRequire(path.join(repoRoot, "package.json"));
const { chromium } = requireFromRepo("playwright");

if (!sourceRevision) {
  throw new Error("NATIVE_BEVY_SOURCE_REVISION is required; identify the source used to build the frozen browser artifact");
}
for (const requiredPath of [fixtureRoot, fixtureMovie, wasmJsPath, wasmBinaryPath, apiStub, apiReal, ruffleRuntime]) {
  if (!fs.existsSync(requiredPath)) throw new Error(`Required baseline input is missing: ${requiredPath}`);
}

const html = `<!doctype html>
<meta charset="utf-8">
<style>body{margin:0}#stage_canvas_container{width:650px;height:420px;overflow:hidden}</style>
<script>
window.__scriptErrors = [];
window.__onScriptError = (message) => window.__scriptErrors.push(String(message));
window.getAudioContext = () => window._audioContext ||= new AudioContext();
window.dirplayer_RufflePlayer = {config:{allowNetworking:'all'}};
window.__dirplayerFlashConfig = {renderer:'canvas',logLevel:'error'};
</script>
<script src="/ruffle/dirplayer_ruffle.js"></script>
<script type="importmap">{"imports":{"dirplayer-js-api":"/dirplayer-js-api.js","vm-rust":"/pkg/${wasmJs}"}}</script>
<div id="stage_canvas_container"></div>
<script type="module">
import init, * as vm from '/pkg/${wasmJs}';
import * as api from '/dirplayer-js-api.js';
window.vm = vm;
try {
  await init();
  vm.player_create_canvas();
  api.setVmModule(vm);
  window.__wasm_trigger_timeout = vm.trigger_timeout;
  vm.set_stage_size(650, 420);
  vm.set_base_path(new URL('/resources/', location.href).href);
  await vm.load_movie_file(new URL('/resources/spybot-nightfall-incident.dcr', location.href).href, true);
  window.__movieLoaded = true;
} catch (error) {
  window.__scriptErrors.push(String(error?.stack || error));
  window.__movieLoaded = false;
}
</script>`;

const mounts = [
  ["/resources/", fixtureRoot],
  ["/pkg/", runnerRoot],
  ["/ruffle/", ruffleRuntime],
  ["/", runnerRoot],
];
const server = createServer((request, response) => {
  try {
    const url = new URL(request.url || "/", "http://127.0.0.1");
    let filename;
    if (url.pathname === "/") filename = null;
    else {
      const mount = mounts.find(([prefix]) => url.pathname.startsWith(prefix));
      if (!mount) throw new Error(`Unknown URL: ${url.pathname}`);
      const relative = decodeURIComponent(url.pathname.slice(mount[0].length));
      filename = path.resolve(mount[1], relative);
      if (!filename.startsWith(`${path.resolve(mount[1])}${path.sep}`)) {
        throw new Error("Resource path escapes its mount");
      }
    }
    if (filename === null) {
      response.writeHead(200, {"Content-Type": "text/html"});
      response.end(html);
      return;
    }
    const data = fs.readFileSync(filename);
    const contentType = filename.endsWith(".html")
      ? "text/html"
      : filename.endsWith(".js")
        ? "text/javascript"
        : filename.endsWith(".wasm")
          ? "application/wasm"
          : "application/octet-stream";
    response.writeHead(200, {"Content-Type": contentType, "Cache-Control": "no-store"});
    response.end(data);
  } catch (error) {
    response.writeHead(404, {"Content-Type": "text/plain"});
    response.end(String(error));
  }
});

await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
const address = server.address();
if (!address || typeof address === "string") throw new Error("Failed to bind baseline server");
const result = {
  schema_version: 1,
  status: "inconclusive",
  source_revision: null,
  ruffle_revision: null,
  browser: null,
  fixture_root: fixtureRoot,
  fixture_movie: path.relative(fixtureRoot, fixtureMovie),
  fixture_movie_sha256: sha256(fixtureMovie),
  settle_ms: 1000,
  ready_frame: null,
  wasm: {js: wasmJs, js_sha256: sha256(wasmJsPath), wasm: wasmBinary, wasm_sha256: sha256(wasmBinaryPath)},
  sound_cases: null,
  observed_sound_starts: null,
  script_errors: [],
};

let browser;
let page;
try {
  result.source_revision = sourceRevision;
  result.ruffle_revision = execFileSync("git", ["-C", "ruffle", "rev-parse", "HEAD"], {cwd: repoRoot, encoding: "utf8"}).trim();
  browser = await chromium.launch({
    headless: true,
    args: ["--autoplay-policy=no-user-gesture-required", "--mute-audio"],
  });
  result.browser = browser.version();
  const context = await browser.newContext({viewport: {width: 650, height: 420}, deviceScaleFactor: 1});
  page = await context.newPage();
  await page.goto(`http://127.0.0.1:${address.port}/`, {waitUntil: "load", timeout: 30000});
  await page.waitForFunction(() => window.__movieLoaded === true || window.__movieLoaded === false, undefined, {timeout: 120000});
  result.script_errors = await page.evaluate(() => window.__scriptErrors || []);
  if (await page.evaluate(() => window.__movieLoaded !== true)) throw new Error(`Movie failed to load: ${result.script_errors.join("; ")}`);
  await page.waitForFunction(async () => {
    try {
      const datum = JSON.parse(await window.vm.mcp_eval_lingo("_movie.frame"));
      return datum.success && JSON.parse(datum.result_value) === 6;
    } catch {
      return false;
    }
  }, undefined, {timeout: 120000});
  result.ready_frame = 6;
  await page.evaluate(() => window.vm.stop());
  await page.waitForTimeout(result.settle_ms);
  result.sound_cases = await testSoundPlayback(page, "s.select", "s.begin", {audioContext: "director"});
  result.script_errors = await page.evaluate(() => window.__scriptErrors || []);
  if (result.script_errors.length > 0) throw new Error(`Page script errors: ${result.script_errors.join("; ")}`);
  result.status = "pass";
} catch (error) {
  result.error = String(error?.stack || error);
  result.observed_sound_starts = page
    ? await page.evaluate(() => window.soundTestStarts || [])
    : null;
  result.status = "fail";
} finally {
  await browser?.close();
}

server.close();
fs.mkdirSync(path.dirname(outputPath), {recursive: true});
fs.writeFileSync(outputPath, `${JSON.stringify(result, null, 2)}\n`);
console.log(JSON.stringify(result, null, 2));
process.exitCode = result.status === "pass" ? 0 : 1;
