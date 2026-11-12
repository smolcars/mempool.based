export type Health = { ok: true; version: string } | { ok: false; reason: string };

/** Interprets a `/healthz` reply. The body is untrusted until checked. */
export function parseHealth(status: number, body: unknown): Health {
  if (status !== 200) return { ok: false, reason: `the server answered with HTTP ${status}` };
  if (typeof body !== "object" || body === null || !("status" in body) || body.status !== "ok") {
    return { ok: false, reason: "the server sent an unexpected reply" };
  }
  const version = "version" in body && typeof body.version === "string" ? body.version : "unknown";
  return { ok: true, version };
}

export async function fetchHealth(): Promise<Health> {
  try {
    const res = await fetch("/healthz", { headers: { accept: "application/json" } });
    const body: unknown = await res.json().catch(() => null);
    return parseHealth(res.status, body);
  } catch {
    return { ok: false, reason: "the server cannot be reached" };
  }
}
