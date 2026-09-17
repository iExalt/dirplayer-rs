#!/usr/bin/env node
import { spawnSync } from "node:child_process";
import dotenv from "dotenv";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import * as esbuild from "esbuild";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = path.resolve(__dirname, "..");
const VM_RUST_DIR = path.join(REPO_ROOT, "vm-rust");
const ASSET_DIR = path.join(REPO_ROOT, "public");
const RUFFLE_DIR = path.join(ASSET_DIR, "ruffle");
const FLASH_MANAGER_SRC = path.join(REPO_ROOT, "src", "services", "flashPlayerManager.ts");
const TEMPLATE_DIR = path.join(VM_RUST_DIR, "tests", "browser_templates");
const CONFIG_DIR = path.join(VM_RUST_DIR, "tests", "e2e", "configs");
const DOTENV_PATH = path.join(REPO_ROOT, ".env");
const IS_WIN = process.platform === "win32";

const dotenvResult = dotenv.config({ path: DOTENV_PATH, quiet: true });
const loadedEnv = {
  ...(dotenvResult.parsed ?? {}),
  ...process.env,
};
const CARGO_TARGET_DIR = path.resolve(
  REPO_ROOT,
  loadedEnv.CARGO_TARGET_DIR || path.join(VM_RUST_DIR, "target"),
);
const RUNNER_DIR = path.resolve(
  REPO_ROOT,
  loadedEnv.BROWSER_RUNNER_DIR || path.join(CARGO_TARGET_DIR, "browser_runner"),
);

function isSameOrAncestor(candidate, descendant) {
  return (
    candidate === descendant ||
    candidate === path.parse(candidate).root ||
    descendant.startsWith(`${candidate}${path.sep}`)
  );
}

function validateRunnerDirectory() {
  const filesystemRoot = path.parse(REPO_ROOT).root;
  const protectedPaths = [filesystemRoot, REPO_ROOT, VM_RUST_DIR, CARGO_TARGET_DIR];
  if (protectedPaths.some((protectedPath) => isSameOrAncestor(RUNNER_DIR, protectedPath))) {
    throw new Error(`Refusing to remove protected browser runner path: ${RUNNER_DIR}`);
  }
  const marker = path.join(RUNNER_DIR, ".dirplayer-browser-runner");
  if (path.basename(RUNNER_DIR) !== "browser_runner" && !fs.existsSync(marker)) {
    throw new Error(
      `BROWSER_RUNNER_DIR must end in browser_runner or contain ${path.basename(marker)}`,
    );
  }
}

validateRunnerDirectory();
const runtimeEnv = {
  ...loadedEnv,
  CARGO_TARGET_DIR,
  BROWSER_RUNNER_DIR: RUNNER_DIR,
};

// Through the Windows shell an argument with a space splits in two, so a
// checkout under "C:\Users\Some Name\..." handed wasm-bindgen half a path.
function quoted(args) {
  if (!IS_WIN) return args;
  return args.map((a) => (/\s/.test(a) && !/^"/.test(a) ? `"${a}"` : a));
}

function run(cmd, args, opts = {}) {
  const res = spawnSync(cmd, quoted(args), {
    stdio: "inherit",
    shell: IS_WIN,
    env: runtimeEnv,
    ...opts,
  });
  if (res.status !== 0) {
    process.exit(res.status ?? 1);
  }
  return res;
}

function buildBrowserTestWasm() {
  const res = spawnSync(
    "cargo",
    quoted([
      "build",
      "--test",
      "mod",
      "--target",
      "wasm32-unknown-unknown",
      "--release",
      "--locked",
      "--message-format=json-render-diagnostics",
    ]),
    {
      cwd: VM_RUST_DIR,
      env: runtimeEnv,
      encoding: "utf8",
      shell: IS_WIN,
      stdio: ["inherit", "pipe", "inherit"],
      maxBuffer: 64 * 1024 * 1024,
    },
  );
  if (res.error) {
    console.error(`Failed to execute cargo: ${res.error.message}`);
    process.exit(1);
  }
  if (res.status !== 0) {
    if (res.stdout) process.stdout.write(res.stdout);
    process.exit(res.status ?? 1);
  }

  const wasmFiles = [];
  for (const line of (res.stdout || "").split(/\r?\n/)) {
    if (!line.trim()) continue;
    let message;
    try {
      message = JSON.parse(line);
    } catch {
      continue;
    }
    if (
      message.reason !== "compiler-artifact" ||
      message.target?.name !== "mod" ||
      !message.target?.kind?.includes("test")
    ) {
      continue;
    }
    for (const filename of message.filenames || []) {
      if (filename.endsWith(".wasm")) wasmFiles.push(filename);
    }
  }
  const wasmFile = wasmFiles.at(-1);
  if (!wasmFile) {
    console.error("Cargo did not report a mod test wasm artifact");
    process.exit(1);
  }
  return wasmFile;
}

// Separate our own flags from args forwarded to playwright.
const cliArgs = process.argv.slice(2);
const forwardArgs = [];
let updateSnapshots = loadedEnv.SNAPSHOT_UPDATE === "1";
let keepOpen = false;
for (const arg of cliArgs) {
  if (arg === "--update" || arg === "-u") {
    updateSnapshots = true;
  } else if (arg === "--debug") {
    keepOpen = true;
  } else {
    forwardArgs.push(arg);
  }
}

// 1. Build browser tests
console.log("Building browser tests...");
const wasmFile = buildBrowserTestWasm();

// 2. Regenerate the runner directory and JS glue.
fs.rmSync(RUNNER_DIR, { recursive: true, force: true });
fs.mkdirSync(RUNNER_DIR, { recursive: true });
fs.writeFileSync(path.join(RUNNER_DIR, ".dirplayer-browser-runner"), "");
run("wasm-bindgen", [wasmFile, "--out-dir", RUNNER_DIR, "--target", "web"]);

// 3. Identify the generated JS filename (exclude *_bg.js).
const jsBasename = fs
  .readdirSync(RUNNER_DIR)
  .find(
    (f) => f.startsWith("mod-") && f.endsWith(".js") && !f.includes("_bg"),
  );
if (!jsBasename) {
  console.error("wasm-bindgen did not produce a mod-*.js file in", RUNNER_DIR);
  process.exit(1);
}

// 4. Scan TOML test configs for ${VAR_NAME} references and collect values
//    from the current process environment.
const envVars = new Set();
if (fs.existsSync(CONFIG_DIR)) {
  const envVarRe = /\$\{([A-Z0-9_]+)/g;
  for (const entry of fs.readdirSync(CONFIG_DIR)) {
    if (!entry.endsWith(".toml")) continue;
    const contents = fs.readFileSync(path.join(CONFIG_DIR, entry), "utf8");
    let m;
    while ((m = envVarRe.exec(contents))) envVars.add(m[1]);
  }
}
const testEnv = {};
for (const name of envVars) {
  const value = runtimeEnv[name];
  if (value !== undefined && value !== "") testEnv[name] = value;
}
const testEnvJson = JSON.stringify(testEnv);

// 5. Copy the JS API stub and render the HTML template.
fs.copyFileSync(
  path.join(TEMPLATE_DIR, "dirplayer-js-api.js"),
  path.join(RUNNER_DIR, "dirplayer-js-api.js"),
);

// 5.1. Also copy the REAL dirplayer-js-api bridge alongside the stub so
// the stub can re-export plugin-loading functions from it. The stub
// keeps no-op UI callbacks (onMovieLoaded etc.) but delegates xtra
// dispatch (loadExternalXtra, createExternalXtraInstance, etc.) to the
// production implementation — so tests that load SDK plugins exercise
// the real wire path end-to-end. `setVmModule(wasm)` in the HTML
// template glues vm-rust resolution to the bridge.
fs.copyFileSync(
  path.join(REPO_ROOT, "dirplayer-js-api", "index.js"),
  path.join(RUNNER_DIR, "dirplayer-js-api-real.js"),
);

// 5a. Bundle flashPlayerManager.ts for Ruffle integration.
//     The `vm-rust` import is externalized and resolved through the
//     importmap to the test's wasm-bindgen module.
await esbuild.build({
  entryPoints: [FLASH_MANAGER_SRC],
  bundle: true,
  format: "esm",
  target: "es2020",
  external: ["vm-rust"],
  outfile: path.join(RUNNER_DIR, "flashPlayerManager.bundle.js"),
  logLevel: "info",
});

// 5b. Copy the Ruffle runtime into the runner so ruffle.js can load
//     its wasm chunk from a sibling path.
if (fs.existsSync(RUFFLE_DIR)) {
  fs.cpSync(RUFFLE_DIR, path.join(RUNNER_DIR, "ruffle"), { recursive: true });
} else {
  console.warn(`Ruffle directory not found at ${RUFFLE_DIR}; Flash members won't render in tests.`);
}

const template = fs.readFileSync(
  path.join(TEMPLATE_DIR, "index.template.html"),
  "utf8",
);
const html = template
  .replaceAll("$WASM_JS_FILE", jsBasename)
  .replaceAll("$TEST_ENV_JSON", testEnvJson)
  .replaceAll("$DEBUG_MODE", keepOpen ? "true" : "false")
  // Optional substring filter: `E2E_FILTER=lore npm run e2e-test-browser`
  // runs only test_* functions whose name contains the string.
  .replaceAll("$TEST_FILTER", (loadedEnv.E2E_FILTER || "").replace(/"/g, ""))
  // `E2E_INTERP_STATS=1` turns on the interpreter opcode/escape counters for
  // the run and writes test-results/interp-stats.txt. OFF by default: the
  // counters add two atomic RMWs per interpreted opcode, which is harmless for
  // counting but perturbs timing, and some snapshots are timing-sensitive.
  .replaceAll("$INTERP_STATS", loadedEnv.E2E_INTERP_STATS === "1" ? "true" : "false");
fs.writeFileSync(path.join(RUNNER_DIR, "index.html"), html);

// 6. Link the asset directory into the runner. Use a junction on Windows so
//    we don't need admin privileges; symlink elsewhere.
const assetsLink = path.join(RUNNER_DIR, "assets");
try {
  fs.rmSync(assetsLink, { recursive: true, force: true });
} catch {
  // ignore
}
try {
  fs.symlinkSync(ASSET_DIR, assetsLink, IS_WIN ? "junction" : "dir");
} catch (e) {
  console.error(
    `Failed to link assets (${ASSET_DIR} -> ${assetsLink}): ${e.message}`,
  );
  process.exit(1);
}

console.log(`Generated test runner in ${RUNNER_DIR}`);

// 7. Run Playwright. SNAPSHOT_UPDATE propagates via process.env.
console.log("Running Playwright tests...");
const playwrightEnv = { ...runtimeEnv };
if (updateSnapshots) playwrightEnv.SNAPSHOT_UPDATE = "1";
if (keepOpen) playwrightEnv.E2E_KEEP_OPEN = "1";

const pw = spawnSync("npx", ["playwright", "test", ...forwardArgs], {
  cwd: REPO_ROOT,
  stdio: "inherit",
  shell: IS_WIN,
  env: playwrightEnv,
});

// 8. Always generate the HTML snapshot report regardless of test outcome.
spawnSync(
  "node",
  quoted([
    path.join(__dirname, "generate-snapshot-report.mjs"),
    path.join(VM_RUST_DIR, "tests", "snapshots"),
    path.join(REPO_ROOT, "test-results", "snapshot-report"),
  ]),
  { cwd: REPO_ROOT, stdio: "inherit", shell: IS_WIN },
);

process.exit(pw.status ?? 1);
