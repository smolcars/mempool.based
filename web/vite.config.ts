import solid from "vite-plugin-solid";
import { defineConfig } from "vitest/config";

// The dev server proxies the Rust server, found where the server itself listens.
const server = `http://${process.env.LISTEN_ADDR ?? "127.0.0.1:8080"}`;

export default defineConfig({
  plugins: [solid()],
  // Unit tests cover pure logic; vite-plugin-solid would otherwise ask for jsdom.
  test: { environment: "node" },
  // No data: URIs, so the strict CSP (default-src 'self') holds for every asset.
  build: { assetsInlineLimit: 0 },
  server: {
    proxy: {
      "^/(api|app)(/|$)": { target: server, ws: true },
      "/healthz": server,
    },
  },
});
