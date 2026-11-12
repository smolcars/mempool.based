import { describe, expect, it } from "vitest";
import { parseConfig } from "./config";

describe("parseConfig", () => {
  it("keeps known networks and the version", () => {
    expect(
      parseConfig({ networks: ["mainnet", "signet", "liquid", 3], version: "0.1.0" }),
    ).toStrictEqual({
      networks: ["mainnet", "signet"],
      version: "0.1.0",
    });
  });

  it("rejects malformed bodies", () => {
    for (const body of [null, "x", [], {}, { networks: "mainnet" }]) {
      expect(parseConfig(body)).toBeNull();
    }
    expect(parseConfig({ networks: [] })).toStrictEqual({ networks: [], version: "" });
  });
});
