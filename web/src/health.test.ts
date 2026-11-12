import { describe, expect, it } from "vitest";
import { parseHealth } from "./health";

describe("parseHealth", () => {
  it("accepts an ok reply", () => {
    expect(parseHealth(200, { status: "ok", version: "0.1.0" })).toEqual({
      ok: true,
      version: "0.1.0",
    });
  });

  it("tolerates a missing or non-string version", () => {
    expect(parseHealth(200, { status: "ok", version: 7 })).toEqual({
      ok: true,
      version: "unknown",
    });
  });

  it("rejects error statuses and malformed bodies", () => {
    expect(parseHealth(503, { status: "ok" }).ok).toBe(false);
    for (const body of [null, "ok", [], { status: "down" }, {}]) {
      expect(parseHealth(200, body).ok).toBe(false);
    }
  });
});
