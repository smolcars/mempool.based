import { afterEach, describe, expect, it, vi } from "vitest";
import { ApiError, clearApiCache, errorMessage, fetchApi, isImmutable, isNotFound } from "./api";

function reply(status: number, body: string, headers: Record<string, string> = {}) {
  return new Response(body, { status, headers });
}

afterEach(() => {
  vi.unstubAllGlobals();
  clearApiCache();
});

describe("errorMessage", () => {
  it("uses our server's error body as a sentence", () => {
    expect(errorMessage(404, { error: "not found" })).toBe("Not found.");
    expect(errorMessage(400, { error: "Invalid txid." })).toBe("Invalid txid.");
  });

  it("falls back to the status for anything else", () => {
    expect(errorMessage(404, "Transaction not found")).toBe("Not found.");
    expect(errorMessage(429, null)).toBe("Too many requests. Try again in a moment.");
    expect(errorMessage(500, { error: 7 })).toBe("The server could not answer.");
    expect(errorMessage(418, { error: "  " })).toBe("HTTP 418.");
  });
});

describe("isImmutable", () => {
  it("keeps only replies marked as never changing", () => {
    expect(isImmutable("public, max-age=31536000, immutable")).toBe(true);
    expect(isImmutable("max-age=86400")).toBe(true);
    expect(isImmutable("max-age=10")).toBe(false);
    expect(isImmutable("no-store, immutable")).toBe(false);
    expect(isImmutable("no-cache")).toBe(false);
    expect(isImmutable(null)).toBe(false);
  });
});

describe("fetchApi", () => {
  it("prefixes the network and parses JSON", async () => {
    const fetch = vi.fn(async () => reply(200, '{"height":5}'));
    vi.stubGlobal("fetch", fetch);
    await expect(fetchApi("signet", "/block/ab")).resolves.toEqual({ height: 5 });
    expect(fetch).toHaveBeenCalledWith("/signet/api/block/ab", expect.anything());
  });

  it("reads text replies", async () => {
    vi.stubGlobal("fetch", async () => reply(200, "975411\n"));
    await expect(fetchApi("mainnet", "/blocks/tip/height", { as: "text" })).resolves.toBe("975411");
  });

  it("maps error bodies to ApiError", async () => {
    vi.stubGlobal("fetch", async () => reply(404, '{"error":"not found"}'));
    const e = await fetchApi("mainnet", "/tx/00").catch((err: unknown) => err);
    expect(e).toBeInstanceOf(ApiError);
    expect(isNotFound(e)).toBe(true);
    expect((e as ApiError).message).toBe("Not found.");
  });

  it("reports an unreachable server and an unreadable reply", async () => {
    vi.stubGlobal("fetch", async () => {
      throw new TypeError("Failed to fetch");
    });
    await expect(fetchApi("mainnet", "/x")).rejects.toMatchObject({ status: 0 });
    vi.stubGlobal("fetch", async () => reply(200, "<html>"));
    await expect(fetchApi("mainnet", "/x")).rejects.toMatchObject({ status: 200 });
  });

  it("passes an abort through instead of reporting it as an error", async () => {
    const controller = new AbortController();
    vi.stubGlobal("fetch", (_url: string, init: RequestInit) => {
      return new Promise((_resolve, reject) => {
        init.signal?.addEventListener("abort", () => reject(init.signal?.reason));
      });
    });
    const pending = fetchApi("mainnet", "/x", { signal: controller.signal });
    controller.abort();
    await expect(pending).rejects.not.toBeInstanceOf(ApiError);
  });

  it("caches immutable replies and nothing else", async () => {
    const fetch = vi.fn(async (url: string) =>
      url.endsWith("/block/ab")
        ? reply(200, "1", { "cache-control": "public, max-age=31536000, immutable" })
        : reply(200, "2", { "cache-control": "max-age=5" }),
    );
    vi.stubGlobal("fetch", fetch);
    await fetchApi("mainnet", "/block/ab");
    await fetchApi("mainnet", "/block/ab");
    await fetchApi("mainnet", "/blocks");
    await fetchApi("mainnet", "/blocks");
    // Same path on another network is another resource.
    await fetchApi("signet", "/block/ab");
    expect(fetch.mock.calls.map((c) => c[0])).toEqual([
      "/api/block/ab",
      "/api/blocks",
      "/api/blocks",
      "/signet/api/block/ab",
    ]);
  });
});
