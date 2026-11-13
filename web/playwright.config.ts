import { defineConfig, devices } from "@playwright/test";
import type { RegtestRpc } from "./e2e/fixtures";

// `bun run e2e` (scripts/e2e.sh) starts the node and the server, then sets these.
const baseURL = process.env["E2E_BASE_URL"];
if (!baseURL) throw new Error("E2E_BASE_URL is not set: run the browser tests with `bun run e2e`");

// The run's own node, for the tests that mine; e2e.sh exports these from `regtest.py env`.
const env = process.env;
const regtestRpc: RegtestRpc | null = env["REGTEST_RPC_PORT"]
  ? {
      url: `http://${env["REGTEST_RPC_HOST"] ?? "127.0.0.1"}:${env["REGTEST_RPC_PORT"]}/`,
      user: env["REGTEST_RPC_USER"] ?? "",
      password: env["REGTEST_RPC_PASSWORD"] ?? "",
    }
  : null;

// The engine and width the suite runs in: `E2E_ENGINE=webkit|firefox` and `E2E_PHONE=1`
// (docs/FRONTEND.md "Other engines"). Pull requests run the default, Chromium at desktop width.
const engine = env["E2E_ENGINE"] ?? "chromium";
const phone = env["E2E_PHONE"] === "1";
const DEVICES: Record<string, { desktop: string; phone: string }> = {
  chromium: { desktop: "Desktop Chrome", phone: "Pixel 7" },
  webkit: { desktop: "Desktop Safari", phone: "iPhone 15" },
  // Firefox has no mobile emulation (`isMobile`), so its phone is a phone-sized touch window at a
  // phone's pixel density.
  firefox: { desktop: "Desktop Firefox", phone: "Desktop Firefox" },
};
const names = DEVICES[engine];
if (!names) throw new Error(`E2E_ENGINE is ${engine}: use chromium, webkit or firefox`);
const device = devices[phone ? names.phone : names.desktop];
if (!device) throw new Error(`Playwright has no device named for ${engine}`);
const use = {
  ...(phone && engine === "firefox"
    ? { ...device, viewport: { width: 393, height: 659 }, deviceScaleFactor: 3, hasTouch: true }
    : device),
  // Firefox on Linux allows WebGL2 only if a GPU probe at launch answers within 4 s; on a busy
  // machine it can miss that, and the browser then has no WebGL2 for its whole life
  // (docs/FRONTEND.md "Other engines"). Other engines ignore Firefox's prefs.
  launchOptions: { firefoxUserPrefs: { "webgl.force-enabled": true } },
};
const project = phone ? `${engine}-phone` : engine;

export default defineConfig<{ regtestRpc: RegtestRpc | null }>({
  testDir: "e2e",
  testMatch: "*.e2e.ts",
  outputDir: process.env["E2E_OUTPUT_DIR"] ?? "test-results",
  fullyParallel: true,
  forbidOnly: !!process.env["CI"],
  reporter: "list",
  use: { baseURL, trace: "retain-on-failure" },
  projects: [
    // Tests share one node and run in parallel, so they never change its chain.
    { name: project, use, testIgnore: "*.mine.e2e.ts" },
    // Tests that mine run after every other test has finished, one at a time (docs/FRONTEND.md).
    {
      name: "mines",
      use: { ...use, regtestRpc },
      testMatch: "*.mine.e2e.ts",
      dependencies: [project],
      fullyParallel: false,
      workers: 1,
    },
  ],
});
