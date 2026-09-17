import fs from "node:fs";
import path from "node:path";

export function baselineRunnerInputs(repoRoot, env = process.env) {
  const targetRoot = path.resolve(repoRoot, env.CARGO_TARGET_DIR || "vm-rust/target");
  const runnerRoot = path.resolve(repoRoot, env.BROWSER_RUNNER_DIR || path.join(targetRoot, "browser_runner"));
  if (!fs.existsSync(runnerRoot)) {
    throw new Error(`Required browser runner is missing: ${runnerRoot}; run the focused browser harness first with the same CARGO_TARGET_DIR/BROWSER_RUNNER_DIR`);
  }
  const entries = fs.readdirSync(runnerRoot);
  const unique = (suffix) => {
    const matches = entries.filter((entry) => entry.startsWith("mod-") && entry.endsWith(suffix) && !entry.endsWith("_bg.js"));
    if (matches.length !== 1) throw new Error(`Expected one mod-*${suffix} under ${runnerRoot}; found ${matches.join(", ") || "none"}`);
    return matches[0];
  };
  const wasmJs = unique(".js");
  const wasmBinary = unique("_bg.wasm");
  if (wasmJs.replace(/\.js$/, "") !== wasmBinary.replace(/_bg\.wasm$/, "")) {
    throw new Error(`WASM JS/binary stems differ: ${wasmJs} vs ${wasmBinary}`);
  }
  return {runnerRoot, wasmJs, wasmBinary};
}
