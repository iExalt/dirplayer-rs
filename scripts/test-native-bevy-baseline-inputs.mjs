import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import {baselineRunnerInputs} from "./native-bevy-baseline-inputs.mjs";

test("baseline runner paths agree with browser harness and validate artifact pairs", () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "baseline-inputs-"));
  try {
    assert.throws(() => baselineRunnerInputs(root, {}), /run the focused browser harness first/);
    for (const [relative, env] of [["vm-rust/target/browser_runner", {}], ["target/browser_runner", {CARGO_TARGET_DIR: "target"}], ["custom", {BROWSER_RUNNER_DIR: "custom", CARGO_TARGET_DIR: "ignored"}]]) {
      const runner = path.join(root, relative);
      fs.mkdirSync(runner, {recursive: true});
      fs.writeFileSync(path.join(runner, "mod-one.js"), "");
      fs.writeFileSync(path.join(runner, "mod-one_bg.wasm"), "");
      assert.equal(baselineRunnerInputs(root, env).runnerRoot, runner);
      assert.equal(baselineRunnerInputs(root, {...env, BROWSER_RUNNER_DIR: runner}).runnerRoot, runner);
      fs.renameSync(path.join(runner, "mod-one_bg.wasm"), path.join(runner, "mod-two_bg.wasm"));
      assert.throws(() => baselineRunnerInputs(root, env), /stems differ/);
      fs.writeFileSync(path.join(runner, "mod-two.js"), "");
      assert.throws(() => baselineRunnerInputs(root, env), /Expected one/);
    }
  } finally {
    fs.rmSync(root, {recursive: true, force: true});
  }
});
