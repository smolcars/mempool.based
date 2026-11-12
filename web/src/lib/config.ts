// What this server serves, from GET /app/config (crates/server/src/app_config.rs).
import { isNetwork, type Network } from "./network";

export interface AppConfig {
  /** Enabled networks, in the server's order. */
  networks: Network[];
  version: string;
}

/** Reads the reply, keeping only known networks. The body is untrusted until checked. */
export function parseConfig(body: unknown): AppConfig | null {
  if (typeof body !== "object" || body === null) return null;
  const networks = "networks" in body && Array.isArray(body.networks) ? body.networks : null;
  if (!networks) return null;
  const version = "version" in body && typeof body.version === "string" ? body.version : "";
  // oxlint-disable-next-line unicorn/no-array-callback-reference -- isNetwork takes one argument
  return { networks: networks.filter(isNetwork), version };
}

/** The server's config, or null when it cannot be had; the shell then offers only the current network. */
export async function fetchConfig(): Promise<AppConfig | null> {
  try {
    const res = await fetch("/app/config", { headers: { accept: "application/json" } });
    return res.ok ? parseConfig(await res.json()) : null;
  } catch {
    return null;
  }
}
