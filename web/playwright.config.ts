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
    {
      name: "chromium",
      use: devices["Desktop Chrome"],
      testIgnore: ["*.mine.e2e.ts", "*.minute.e2e.ts"],
    },
    // Tests that wait for the server's first recorded minute start at once and wait beside the rest.
    { name: "minute", use: devices["Desktop Chrome"], testMatch: "*.minute.e2e.ts" },
    // Tests that mine run after every other test has finished, one at a time (docs/FRONTEND.md).
    {
      name: "mines",
      use: { ...devices["Desktop Chrome"], regtestRpc },
      testMatch: "*.mine.e2e.ts",
      dependencies: ["chromium"],
      fullyParallel: false,
      workers: 1,
    },
  ],
});
