import { defineConfig, devices } from "@playwright/test";

// `bun run e2e` (scripts/e2e.sh) starts the node and the server, then sets these.
const baseURL = process.env.E2E_BASE_URL;
if (!baseURL) throw new Error("E2E_BASE_URL is not set: run the browser tests with `bun run e2e`");

export default defineConfig({
  testDir: "e2e",
  testMatch: "*.e2e.ts",
  outputDir: process.env.E2E_OUTPUT_DIR ?? "test-results",
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  reporter: "list",
  use: { baseURL, trace: "retain-on-failure" },
  projects: [{ name: "chromium", use: devices["Desktop Chrome"] }],
});
