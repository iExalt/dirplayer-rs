import { test, expect } from "@playwright/test";
import { spawn, spawnSync } from "child_process";
import * as fs from "fs";
import * as path from "path";
import { fileURLToPath } from "url";
import { PNG } from "pngjs";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const SNAPSHOTS_BASE = path.join(__dirname, "..", "snapshots");
const UPDATE_SNAPSHOTS = process.env.SNAPSHOT_UPDATE === "1";
let multiuserServer: ReturnType<typeof spawn> | undefined;
const MULTIUSER_TEST_NAME = "test_multiuser_socket_lifecycle";
const FILEIO_TEST_NAME = "test_fileio_open_remote";
const FIXTURE_TEST_NAMES = [MULTIUSER_TEST_NAME, FILEIO_TEST_NAME];
const RUN_MULTIUSER_FIXTURE = (() => {
  const filter = process.env.E2E_FILTER;
  if (!filter) return true;
  return filter
    .split(",")
    .map((part) => part.trim().toLowerCase())
    .filter(Boolean)
    .some((part) => FIXTURE_TEST_NAMES.some((name) => name.includes(part)));
})();

async function startMultiuserFixture(): Promise<number> {
  const helper = path.join(__dirname, "multiuser-server.mjs");
  const child = spawn(process.env.BUN_BIN || "bun", [helper], {
    stdio: ["ignore", "pipe", "inherit"],
  });
  multiuserServer = child;
  return new Promise((resolve, reject) => {
    let output = "";
    let settled = false;
    const settle = (error: Error | null, port?: number) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      child.stdout?.off("data", onData);
      child.off("error", onError);
      child.off("exit", onExit);
      if (error) reject(error);
      else resolve(port!);
    };
    const timer = setTimeout(() => {
      child.kill("SIGTERM");
      settle(new Error(`multiuser fixture did not start within 5 seconds: ${output}`));
    }, 5000);
    const onData = (chunk: Buffer | string) => {
      output += chunk.toString();
      const match = output.match(/MULTIUSER_TEST_SERVER_READY (\d+)/);
      if (match) {
        settle(null, Number(match[1]));
      }
    };
    const onError = (error: Error) => settle(error);
    const onExit = (code: number | null, signal: string | null) => {
      settle(new Error(`multiuser fixture exited before ready (${code ?? signal}): ${output}`));
    };
    child.stdout?.on("data", onData);
    child.once("error", onError);
    child.once("exit", onExit);
  });
}

test.beforeEach(async ({ page }) => {
  if (!RUN_MULTIUSER_FIXTURE) return;
  const port = await startMultiuserFixture();
  await page.addInitScript((fixturePort) => {
    (window as any).__multiuserTestWsBase = `ws://127.0.0.1:${fixturePort}`;
    (window as any).__fileIoTestHttpBase = `http://127.0.0.1:${fixturePort}`;
  }, port);
});

test.afterEach(async () => {
  const child = multiuserServer;
  multiuserServer = undefined;
  if (!child) return;
  const waitForExit = (timeoutMs: number) => new Promise<boolean>((resolve) => {
    if (child.exitCode !== null || child.signalCode !== null) {
      resolve(true);
      return;
    }
    const timer = setTimeout(() => {
      child.off("exit", onExit);
      resolve(false);
    }, timeoutMs);
    const onExit = () => {
      clearTimeout(timer);
      resolve(true);
    };
    child.once("exit", onExit);
  });
  child.kill("SIGTERM");
  if (!(await waitForExit(1000))) {
    child.kill("SIGKILL");
    await waitForExit(1000);
  }
});

interface TestResult {
  name: string;
  status: "pass" | "fail";
  error?: string;
}

interface TestResults {
  tests: TestResult[];
  passed: number;
  failed: number;
  done: boolean;
}

function compareSnapshots(
  actualPath: string,
  referencePath: string,
  diffPath: string | null,
  pixelTolerance: number = 0
): { diffRatio: number; diffPixels: number; totalPixels: number } {
  const actual = PNG.sync.read(fs.readFileSync(actualPath));
  const reference = PNG.sync.read(fs.readFileSync(referencePath));

  if (actual.width !== reference.width || actual.height !== reference.height) {
    throw new Error(
      `Dimensions differ: actual ${actual.width}x${actual.height} vs reference ${reference.width}x${reference.height}`
    );
  }

  const totalPixels = actual.width * actual.height;
  let diffPixels = 0;

  // Build a diff image: changed pixels shown in red on a dimmed reference
  const diffImg = diffPath ? new PNG({ width: actual.width, height: actual.height }) : null;

  for (let i = 0; i < totalPixels; i++) {
    const off = i * 4;
    const dr = Math.abs(actual.data[off] - reference.data[off]);
    const dg = Math.abs(actual.data[off + 1] - reference.data[off + 1]);
    const db = Math.abs(actual.data[off + 2] - reference.data[off + 2]);
    const da = Math.abs(actual.data[off + 3] - reference.data[off + 3]);
    const changed = Math.max(dr, dg, db, da) > pixelTolerance;
    if (changed) diffPixels++;

    if (diffImg) {
      if (changed) {
        // Red highlight with intensity proportional to the diff
        diffImg.data[off] = 255;
        diffImg.data[off + 1] = 0;
        diffImg.data[off + 2] = 0;
        diffImg.data[off + 3] = 255;
      } else {
        // Dimmed reference pixel
        diffImg.data[off] = reference.data[off] >> 2;
        diffImg.data[off + 1] = reference.data[off + 1] >> 2;
        diffImg.data[off + 2] = reference.data[off + 2] >> 2;
        diffImg.data[off + 3] = reference.data[off + 3];
      }
    }
  }

  if (diffImg && diffPixels > 0 && diffPath) {
    fs.mkdirSync(path.dirname(diffPath), { recursive: true });
    fs.writeFileSync(diffPath, new Uint8Array(PNG.sync.write(diffImg)));
  } else if (diffPath && fs.existsSync(diffPath)) {
    fs.unlinkSync(diffPath);
  }

  return { diffRatio: diffPixels / totalPixels, diffPixels, totalPixels };
}

function processSnapshot(
  suitePath: string,
  name: string,
  base64data: string,
  maxDiffRatio: number,
  pixelTolerance: number = 0
): string {
  const slashIdx = suitePath.indexOf("/");
  const suite = slashIdx >= 0 ? suitePath.substring(0, slashIdx) : suitePath;
  const testName =
    slashIdx >= 0 ? suitePath.substring(slashIdx + 1) : "default";

  const outputDir = path.join(SNAPSHOTS_BASE, "output", suite, "browser", testName);
  const referenceDir = path.join(SNAPSHOTS_BASE, "reference", suite, "browser", testName);
  fs.mkdirSync(outputDir, { recursive: true });
  fs.mkdirSync(referenceDir, { recursive: true });

  const fileName = `${name}.png`;
  const outputPath = path.join(outputDir, fileName);
  const referencePath = path.join(referenceDir, fileName);

  fs.writeFileSync(outputPath, new Uint8Array(Buffer.from(base64data, "base64")));
  console.log(`Saved: ${suite}/browser/${testName}/${fileName}`);

  if (UPDATE_SNAPSHOTS) {
    fs.writeFileSync(
      referencePath,
      new Uint8Array(Buffer.from(base64data, "base64"))
    );
    return "reference updated";
  }

  if (!fs.existsSync(referencePath)) {
    return "no reference";
  }

  const diffDir = path.join(SNAPSHOTS_BASE, "diff", suite, "browser", testName);
  const diffPath = path.join(diffDir, fileName);
  const diff = compareSnapshots(outputPath, referencePath, diffPath, pixelTolerance);
  if (diff.diffRatio > maxDiffRatio) {
    throw new Error(
      `Snapshot '${suite}/browser/${testName}/${name}' differs from reference: ` +
        `${(diff.diffRatio * 100).toFixed(4)}% pixels changed ` +
        `(${diff.diffPixels}/${diff.totalPixels}, threshold: ${(maxDiffRatio * 100).toFixed(4)}%)`
    );
  }
  // Snapshot passed — remove any stale diff so the report doesn't flag it as changed.
  if (fs.existsSync(diffPath)) fs.unlinkSync(diffPath);
  return `${(diff.diffRatio * 100).toFixed(3)}% diff`;
}

test("browser e2e tests", async ({ page }) => {
  const snapshotErrors: string[] = [];
  const pageLifecycle: string[] = ["before-goto"];
  const pageErrors: string[] = [];
  const failedRequests: string[] = [];
  const badResponses: string[] = [];

  page.on("domcontentloaded", () => {
    pageLifecycle.push("domcontentloaded");
  });
  page.on("pageerror", (error) => {
    const detail = error.stack ? `${error.message}\n${error.stack}` : error.message;
    pageErrors.push(detail);
    console.log(`[browser-e2e] pageerror: ${detail}`);
  });
  page.on("requestfailed", (request) => {
    const failure = request.failure()?.errorText ?? "unknown request failure";
    const detail = `${request.method()} ${request.url()} — ${failure}`;
    failedRequests.push(detail);
    console.log(`[browser-e2e] requestfailed: ${detail}`);
  });
  page.on("response", (response) => {
    if (response.status() < 400) return;
    const detail = `${response.status()} ${response.request().method()} ${response.url()}`;
    badResponses.push(detail);
    console.log(`[browser-e2e] response>=400: ${detail}`);
  });

  const capturePageState = async (): Promise<string> => {
    try {
      const state = await page.evaluate(() => {
        const win = window as any;
        const output = document.getElementById("output");
        return {
          url: window.location.href,
          readyState: document.readyState,
          outputText: output?.textContent?.slice(0, 12000) ?? "",
          bodyText: document.body?.textContent?.slice(0, 12000) ?? "",
          testResults: win.__testResults ?? null,
          testPanic: win.__testPanic ?? null,
          scriptErrors: win.__scriptErrors ?? [],
          currentPhase: win.__dirplayerNestedPhase ?? win.__testPhase ?? null,
        };
      });
      return JSON.stringify({ pageLifecycle, pageErrors, failedRequests, badResponses, state }, null, 2);
    } catch (error) {
      return JSON.stringify({
        pageLifecycle,
        pageErrors,
        failedRequests,
        badResponses,
        evaluateError: error instanceof Error ? error.message : String(error),
      }, null, 2);
    }
  };

  // Expose snapshot handler so snapshots are saved as they're taken
  await page.exposeFunction(
    "__playwrightSaveSnapshot",
    async (suite: string, name: string, data: string, maxDiffRatio: number, pixelTolerance: number = 0) => {
      try {
        const status = processSnapshot(suite, name, data, maxDiffRatio, pixelTolerance);
        return { ok: true, status };
      } catch (err: any) {
        const msg = err?.message ?? String(err);
        snapshotErrors.push(msg);
        return { ok: false, status: msg };
      }
    }
  );

  // `E2E_CONSOLE=1` forwards the page console to the terminal (optionally
  // filtered by a substring, e.g. `E2E_CONSOLE=PROBE`) — the only way to see
  // `log_test_action` / diagnostic output from inside the wasm test.
  const consoleFilter = process.env.E2E_CONSOLE;
  if (consoleFilter) {
    const needle = consoleFilter === "1" ? "" : consoleFilter;
    page.on("console", (msg) => {
      const text = msg.text();
      if (!needle || text.includes(needle)) console.log(`[page] ${text}`);
    });
  }

  const navigation = await page.goto("/index.html");
  pageLifecycle.push("goto-resolved");
  console.log(
    `[browser-e2e] harness-start page.goto resolved status=${navigation?.status() ?? "no-response"} ` +
      `url=${page.url()}`
  );
  console.log(`[browser-e2e] harness-start state=${await capturePageState()}`);

  // Stop waiting as soon as the harness finishes, a panic hook reports a
  // Rust panic, or the page accumulates script errors.
  let handle: Awaited<ReturnType<typeof page.waitForFunction>>;
  try {
    handle = await page.waitForFunction(
      () => {
        const win = window as any;
        return (
          win.__testResults?.done === true ||
          typeof win.__testPanic === "string" ||
          (Array.isArray(win.__scriptErrors) && win.__scriptErrors.length > 0)
        );
      },
      { timeout: 900_000 }
    );
  } catch (error) {
    const diagnostic = await capturePageState();
    console.log(`[browser-e2e] harness wait failed: ${diagnostic}`);
    throw new Error(
      `browser harness did not publish completion: ${error instanceof Error ? error.message : String(error)}\n${diagnostic}`
    );
  }
  await handle.dispose();

  const [testResults, panicMessage, scriptErrors, interpStats] = await Promise.all([
    page.evaluate(() => ((window as any).__testResults ?? null) as TestResults | null),
    page.evaluate(() => ((window as any).__testPanic ?? null) as string | null),
    page.evaluate(() => ((window as any).__scriptErrors ?? []) as string[]),
    page.evaluate(() => ((window as any).__interpStats ?? null) as string | null),
  ]);

  // Interpreter opcode/escape counters, accumulated across every test in the
  // suite (all of them share one wasm instance). Written unconditionally so a
  // failing run still yields the histogram.
  if (interpStats) {
    const statsDir = path.resolve(__dirname, "../../..", "test-results");
    fs.mkdirSync(statsDir, { recursive: true });
    const statsPath = path.join(statsDir, "interp-stats.txt");
    fs.writeFileSync(statsPath, interpStats);
    console.log(`\nInterpreter stats written to ${statsPath}`);
    console.log(interpStats);
  }

  // Collect all errors before acting on them so keep-open can fire first.
  const errors: string[] = [];

  if (panicMessage) {
    errors.push(`Rust panic during browser test:\n${panicMessage}`);
  }

  if (scriptErrors.length > 0) {
    console.log(`\n${scriptErrors.length} script error(s):`);
    for (const err of scriptErrors) {
      console.log(`  ✗ ${err}`);
    }
    errors.push(`${scriptErrors.length} script error(s) during test:\n${scriptErrors.join("\n")}`);
  }

  if (!testResults) {
    errors.push("Browser test harness exited without publishing test results.");
  }

  if (testResults) {
    for (const t of testResults.tests) {
      if (t.status === "pass") {
        console.log(`✓ ${t.name}`);
      } else {
        console.log(`✗ ${t.name}: ${t.error}`);
      }
    }
    console.log(
      `${testResults.passed} passed, ${testResults.failed} failed`
    );
  }

  if (snapshotErrors.length > 0) {
    errors.push(
      `${snapshotErrors.length} snapshot comparison failure(s):\n${snapshotErrors.join("\n")}`
    );
  }

  // In debug mode, generate the snapshot report while the browser is still
  // open so the user doesn't need to Ctrl+C to trigger it.
  if (process.env.E2E_KEEP_OPEN === "1") {
    const repoRoot = path.resolve(__dirname, "../../..");
    spawnSync(
      "node",
      [
        path.join(repoRoot, "scripts", "generate-snapshot-report.mjs"),
        path.join(repoRoot, "vm-rust", "tests", "snapshots"),
        path.join(repoRoot, "test-results", "snapshot-report"),
      ],
      { stdio: "inherit" }
    );
  }

  // In debug mode, keep the browser open so the log can be inspected.
  if (process.env.E2E_KEEP_OPEN === "1" && errors.length > 0) {
    console.log("\nKeeping browser open for inspection — press Ctrl+C to exit.");
    await new Promise<void>(() => {});
  }

  if (errors.length > 0) {
    throw new Error(errors.join("\n\n"));
  }

  // Assert all tests passed
  expect(testResults!.tests.length).toBeGreaterThan(0);
  const requestedFilters = (process.env.E2E_FILTER || "")
    .toLowerCase()
    .split(",")
    .map((filter) => filter.trim())
    .filter(Boolean);
  if (requestedFilters.length > 0) {
    expect(
      testResults!.tests.every((test) =>
        requestedFilters.some((filter) => test.name.toLowerCase().includes(filter))
      )
    ).toBe(true);
  }
  expect(testResults!.failed).toBe(0);
});
