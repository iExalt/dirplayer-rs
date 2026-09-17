#!/usr/bin/env node

import { createHash } from "node:crypto";
import { createServer } from "node:http";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { createRequire } from "node:module";
import { execFileSync } from "node:child_process";

const scriptDir = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(scriptDir, "..");
const ruffleDist = path.join(repoRoot, "ruffle", "web", "packages", "selfhosted", "dist");
const fixturePath = path.join(
  repoRoot,
  "ruffle",
  "tests",
  "tests",
  "swfs",
  "avm1",
  "localconnection",
  "test.swf",
);
const ruffleScriptPath = path.join(ruffleDist, "dirplayer_ruffle.js");
const wasmPaths = fs.existsSync(ruffleDist)
  ? fs.readdirSync(ruffleDist)
      .filter((entry) => entry.endsWith(".wasm"))
      .map((entry) => path.join(ruffleDist, entry))
  : [];
if (!fs.existsSync(ruffleScriptPath)) {
  throw new Error(`Selfhosted Ruffle bundle is missing: ${ruffleScriptPath}`);
}
if (wasmPaths.length !== 1) {
  throw new Error(`Expected exactly one selfhosted Ruffle wasm, found ${wasmPaths.length}`);
}
if (!fs.existsSync(fixturePath)) {
  throw new Error(`AVM1 LocalConnection fixture is missing: ${fixturePath}`);
}

const requireFromRepo = createRequire(path.join(repoRoot, "package.json"));
const { chromium } = requireFromRepo("playwright");
const ownerA = "avm1-owner-a:g1";
const ownerB = "avm1-owner-b:g1";
const timeoutMs = 30_000;

function sha256(filePath) {
  return createHash("sha256").update(fs.readFileSync(filePath)).digest("hex");
}

function contentType(filePath) {
  if (filePath.endsWith(".js")) return "text/javascript; charset=utf-8";
  if (filePath.endsWith(".wasm")) return "application/wasm";
  if (filePath.endsWith(".swf")) return "application/x-shockwave-flash";
  return "application/octet-stream";
}

const html = `<!doctype html>
<meta charset="utf-8">
<style>body { margin: 0; background: #111; } #players { display: flex; gap: 4px; }</style>
<script>
  window.__ownerEvents = [];
  window.__retiredOwnerEvents = [];
  window.__retirementProbeActive = false;
  window.__activeOwnerKeys = new Set([${JSON.stringify(ownerA)}, ${JSON.stringify(ownerB)}]);
  window.dirplayer_RufflePlayer = { config: { allowNetworking: "all", logLevel: "error" } };
  // Capture the same owner-qualified global used by production. This browser
  // regression deliberately records the rebuilt AVM1 producer at this seam;
  // production host routing is covered separately by the bridge harness.
  window.dirplayer_localConnectionSendOwned = function(ownerKey, connectionName, methodName, argsJson) {
    const record = { ownerKey, connectionName, methodName, argsJson };
    if (!window.__activeOwnerKeys.has(ownerKey)) {
      window.__retiredOwnerEvents.push({ ...record, source: window.__retirementProbeActive ? "probe" : "swf" });
      return false;
    }
    window.__ownerEvents.push(record);
    return true;
  };
</script>
<script src="/ruffle/dirplayer_ruffle.js"></script>
<div id="players"></div>`;

const mounts = [
  ["/ruffle/", ruffleDist],
  ["/fixture/", path.dirname(fixturePath)],
];
const server = createServer((request, response) => {
  try {
    const url = new URL(request.url || "/", "http://127.0.0.1");
    let filePath;
    if (url.pathname === "/") {
      response.writeHead(200, { "Content-Type": "text/html; charset=utf-8" });
      response.end(html);
      return;
    }
    const mount = mounts.find(([prefix]) => url.pathname.startsWith(prefix));
    if (!mount) throw new Error(`Unknown URL: ${url.pathname}`);
    const relative = decodeURIComponent(url.pathname.slice(mount[0].length));
    filePath = path.resolve(mount[1], relative);
    const mountRoot = path.resolve(mount[1]);
    if (filePath !== mountRoot && !filePath.startsWith(`${mountRoot}${path.sep}`)) {
      throw new Error("Resource path escapes its mount");
    }
    if (!fs.existsSync(filePath) || !fs.statSync(filePath).isFile()) {
      response.writeHead(404, { "Content-Type": "text/plain; charset=utf-8" });
      response.end("Not found");
      return;
    }
    response.writeHead(200, {
      "Cache-Control": "no-store",
      "Content-Type": contentType(filePath),
    });
    response.end(fs.readFileSync(filePath));
  } catch (error) {
    response.writeHead(400, { "Content-Type": "text/plain; charset=utf-8" });
    response.end(String(error));
  }
});

const evidenceDir = process.env.RUFFLE_OWNER_EVIDENCE ||
  path.join(repoRoot, ".cache", "native-bevy", "evidence", "ruffle-owner-build-20260916");
const evidencePath = path.join(evidenceDir, "avm1-owner-browser.json");
const result = {
  status: "inconclusive",
  fixture: path.relative(repoRoot, fixturePath),
  fixture_sha256: sha256(fixturePath),
  ruffle_script: path.relative(repoRoot, ruffleScriptPath),
  ruffle_script_sha256: sha256(ruffleScriptPath),
  ruffle_wasm: path.relative(repoRoot, wasmPaths[0]),
  ruffle_wasm_sha256: sha256(wasmPaths[0]),
  ruffle_revision: execFileSync("git", ["-C", path.join(repoRoot, "ruffle"), "rev-parse", "HEAD"], {
    cwd: repoRoot,
    encoding: "utf8",
  }).trim(),
  owner_a: ownerA,
  owner_b: ownerB,
  endpoint_mode: "owner-qualified-test-capture",
  production_host_routing_exercised: false,
  owner_events: [],
  post_retirement_owner_events: [],
  retired_owner_events: [],
  page_errors: [],
};

let browser;
let context;
let page;
try {
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  const address = server.address();
  if (!address || typeof address === "string") throw new Error("Failed to bind browser fixture server");

  browser = await chromium.launch({
    headless: true,
    args: ["--autoplay-policy=no-user-gesture-required", "--mute-audio"],
  });
  context = await browser.newContext({ viewport: { width: 640, height: 480 }, deviceScaleFactor: 1 });
  page = await context.newPage();
  page.on("pageerror", (error) => result.page_errors.push(String(error?.stack || error)));
  await page.goto(`http://127.0.0.1:${address.port}/`, { waitUntil: "load", timeout: timeoutMs });
  await page.waitForFunction(() => typeof window.dirplayer_RufflePlayer?.newest === "function", undefined, {
    timeout: timeoutMs,
  });

  const loadPlayer = async (ownerKey) => {
    await page.evaluate(async ({ ownerKey: key }) => {
      const player = window.dirplayer_RufflePlayer.newest().createPlayer();
      if (typeof player.dirplayer_set_owner_key !== "function") {
        throw new Error("Rebuilt Ruffle player lacks dirplayer_set_owner_key");
      }
      player.dirplayer_set_owner_key(key);
      player.style.width = "320px";
      player.style.height = "240px";
      document.querySelector("#players").appendChild(player);
      window.__ownerPlayers ||= new Map();
      window.__ownerPlayers.set(key, player);
      await player.ruffle().load("/fixture/test.swf");
    }, { ownerKey });
  };

  await Promise.all([loadPlayer(ownerA), loadPlayer(ownerB)]);
  await page.waitForFunction(() => {
    const events = window.__ownerEvents || [];
    const hasFixtureSend = (ownerKey) => events.some((event) =>
      event.ownerKey === ownerKey &&
      event.connectionName === "channel" &&
      event.methodName === "test" &&
      typeof event.argsJson === "string",
    );
    return hasFixtureSend("avm1-owner-a:g1") && hasFixtureSend("avm1-owner-b:g1");
  }, undefined, { timeout: timeoutMs });

  const initial = await page.evaluate(() => ({
    events: window.__ownerEvents.slice(),
    pageErrors: window.__pageErrors || [],
  }));
  result.owner_events = initial.events;
  const expectedOwners = new Set([ownerA, ownerB]);
  const unexpectedInitialOwners = initial.events.filter((event) => !expectedOwners.has(event.ownerKey));
  if (unexpectedInitialOwners.length > 0) {
    throw new Error(`AVM1 producer emitted unexpected owner keys: ${JSON.stringify(unexpectedInitialOwners)}`);
  }
  const validFixtureSend = (ownerKey) => initial.events.some((event) => {
    if (event.ownerKey !== ownerKey || event.connectionName !== "channel" || event.methodName !== "test") {
      return false;
    }
    try {
      return JSON.stringify(JSON.parse(event.argsJson)) === "[]";
    } catch {
      return false;
    }
  });
  if (!validFixtureSend(ownerA) || !validFixtureSend(ownerB)) {
    throw new Error("AVM1 fixture did not produce channel/test/[] for both owner-qualified players");
  }

  const beforeRetireEventCount = initial.events.length;
  const beforeBReload = initial.events.filter((event) =>
    event.ownerKey === ownerB && event.connectionName === "channel" && event.methodName === "test",
  ).length;
  await page.evaluate(async (key) => {
    const player = window.__ownerPlayers.get(key);
    window.__activeOwnerKeys.delete(key);
    player.pause();
    player.remove();
    window.__ownerPlayers.delete(key);
  }, ownerA);
  const retiredResult = await page.evaluate(() => {
    window.__retirementProbeActive = true;
    const accepted = window.dirplayer_localConnectionSendOwned(
      "avm1-owner-a:g1",
      "channel",
      "test",
      "[]",
    );
    window.__retirementProbeActive = false;
    return accepted;
  });
  if (retiredResult !== false) throw new Error("Retired owner endpoint accepted a send");

  await page.evaluate(async (key) => {
    const player = window.__ownerPlayers.get(key);
    player.pause();
    await player.ruffle().load("/fixture/test.swf");
  }, ownerB);
  await page.waitForFunction((minimum) => {
    return (window.__ownerEvents || []).filter((event) =>
      event.ownerKey === "avm1-owner-b:g1" &&
      event.connectionName === "channel" &&
      event.methodName === "test",
    ).length > minimum;
  }, beforeBReload, { timeout: timeoutMs });
  await page.waitForTimeout(250);

  const finalState = await page.evaluate(() => ({
    events: window.__ownerEvents.slice(),
    retired: window.__retiredOwnerEvents.slice(),
  }));
  result.owner_events = finalState.events;
  result.retired_owner_events = finalState.retired;
  result.post_retirement_owner_events = finalState.events.slice(beforeRetireEventCount);
  if (result.post_retirement_owner_events.some((event) => event.ownerKey === ownerA)) {
    throw new Error("Retired owner emitted a post-retirement LocalConnection send");
  }
  if (finalState.retired.some((event) => event.source === "swf")) {
    throw new Error("A stale SWF callback reached the retired owner endpoint");
  }
  if (!finalState.retired.some((event) => event.source === "probe")) {
    throw new Error("Retired owner probe was not rejected by the owner-qualified endpoint");
  }
  if (finalState.events.filter((event) =>
    event.ownerKey === ownerB && event.connectionName === "channel" && event.methodName === "test",
  ).length <= beforeBReload) {
    throw new Error("Surviving owner produced no send after the retired peer was removed");
  }
  if (result.page_errors.length > 0) {
    throw new Error(`Browser page errors: ${result.page_errors.join("; ")}`);
  }
  result.status = "pass";
} catch (error) {
  result.status = "fail";
  result.error = String(error?.stack || error);
} finally {
  await context?.close();
  await browser?.close();
  await new Promise((resolve) => server.close(resolve));
  fs.mkdirSync(evidenceDir, { recursive: true });
  fs.writeFileSync(evidencePath, `${JSON.stringify(result, null, 2)}\n`);
}

console.log(JSON.stringify({ ...result, evidence: evidencePath }, null, 2));
process.exitCode = result.status === "pass" ? 0 : 1;
