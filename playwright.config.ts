import { defineConfig } from "@playwright/test";
import { dirname } from "path";
import { fileURLToPath } from "url";

const __dirname = dirname(fileURLToPath(import.meta.url));
const browserRunnerPort = Number(process.env.BROWSER_RUNNER_PORT || "9101");
const reuseBrowserServer = process.env.E2E_REUSE_SERVER !== "0";
const configuredBrowserVideo = process.env.E2E_VIDEO;
const browserVideo: "off" | "on" | "retain-on-failure" | "on-first-retry" =
  configuredBrowserVideo === "off" ||
  configuredBrowserVideo === "on" ||
  configuredBrowserVideo === "retain-on-failure" ||
  configuredBrowserVideo === "on-first-retry"
    ? configuredBrowserVideo
    : process.env.CI
      ? "retain-on-failure"
      : "off";

export default defineConfig({
  testDir: "./vm-rust/tests/browser",
  timeout: 1_800_000,
  use: {
    headless: !!process.env.CI,
    baseURL: `http://127.0.0.1:${browserRunnerPort}`,
    video: browserVideo,
  },
  webServer: {
    command: "node scripts/serve-browser-runner.mjs",
    port: browserRunnerPort,
    cwd: __dirname,
    reuseExistingServer: reuseBrowserServer,
  },
});
