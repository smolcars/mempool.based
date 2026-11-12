import { describe, expect, it } from "vitest";
import { apiBase, networkFromPath, onNetwork, paths, stripNetwork } from "./network";

describe("network paths", () => {
  it("reads the network from the first segment", () => {
    expect(networkFromPath("/")).toBe("mainnet");
    expect(networkFromPath("/tx/ab")).toBe("mainnet");
    expect(networkFromPath("/signet")).toBe("signet");
    expect(networkFromPath("/signet/")).toBe("signet");
    expect(networkFromPath("/testnet4/block/1")).toBe("testnet4");
    expect(networkFromPath("/regtest/tx/ab")).toBe("regtest");
    // Not prefixes: mainnet has none, and look-alikes stay mainnet.
    expect(networkFromPath("/mainnet/tx/ab")).toBe("mainnet");
    expect(networkFromPath("/signetx/tx")).toBe("mainnet");
    expect(networkFromPath("/address/signet")).toBe("mainnet");
  });

  it("strips and adds the prefix symmetrically", () => {
    for (const path of ["/", "/tx/ab", "/block/12/", "/mining"]) {
      for (const net of ["mainnet", "signet", "testnet4", "regtest"] as const) {
        const placed = onNetwork(net, path);
        expect(networkFromPath(placed)).toBe(net);
        expect(stripNetwork(placed)).toBe(path);
      }
    }
    expect(onNetwork("signet", "/")).toBe("/signet");
    expect(stripNetwork("/signet/")).toBe("/");
    expect(stripNetwork("")).toBe("/");
  });

  it("points each network at its own API", () => {
    expect(apiBase("mainnet")).toBe("/api");
    expect(apiBase("testnet4")).toBe("/testnet4/api");
  });

  it("encodes chain data into a single path segment", () => {
    expect(paths.tx("ab")).toBe("/tx/ab");
    expect(paths.block(840000)).toBe("/block/840000");
    // oxlint-disable-next-line eslint/no-script-url -- checks that such a value is escaped
    expect(paths.address("javascript:alert(1)")).toBe("/address/javascript%3Aalert(1)");
    expect(paths.address("../../api/x?y#z")).toBe("/address/..%2F..%2Fapi%2Fx%3Fy%23z");
  });
});
