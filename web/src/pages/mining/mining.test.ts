import { describe, expect, it } from "vitest";
import {
  byTime,
  coverageNote,
  fillWeeks,
  hashrateInterval,
  hashrateSeries,
  isPoolStats,
  isRanking,
  networkHashrate,
  networkNote,
  OTHER_POOLS,
  parsePeriod,
  periodDomain,
  poolLayers,
  type Ranking,
  rewardFigures,
  safeLink,
  sharesOverTime,
} from "./mining";

const DAY = 86_400;

describe("periods", () => {
  it("reads the period from the address, with a week by default", () => {
    expect(parsePeriod("1m").id).toBe("1m");
    expect(parsePeriod(["1y"]).id).toBe("1y");
    expect(parsePeriod("10y").id).toBe("1w");
    expect(parsePeriod(undefined).id).toBe("1w");
  });
  it("shows at least a week of hashrate, and shares only by the month or longer", () => {
    expect(hashrateInterval(parsePeriod("24h"))).toBe("1w");
    expect(hashrateInterval(parsePeriod("1y"))).toBe("1y");
    expect(sharesOverTime(parsePeriod("1w"))).toBe(false);
    expect(sharesOverTime(parsePeriod("1m"))).toBe(true);
  });
  it("draws the whole period, or the data's extent for all time", () => {
    const now = 100 * DAY;
    expect(periodDomain(parsePeriod("1m"), [90 * DAY, 99 * DAY], now)).toStrictEqual([
      70 * DAY,
      now,
    ]);
    expect(periodDomain(parsePeriod("all"), [1, 2], now)).toBeUndefined();
    expect(periodDomain(parsePeriod("1m"), [], now)).toBeUndefined();
  });
});

describe("hashrate and difficulty", () => {
  it("gives each point the difficulty in force, and the one before the first retarget", () => {
    const s = hashrateSeries({
      hashrates: [
        { timestamp: 30, avgHashrate: 3 },
        { timestamp: 10, avgHashrate: 1 },
        { timestamp: 20, avgHashrate: 2 },
      ],
      difficulty: [{ time: 15, height: 2016, difficulty: 110, adjustment: 1.1 }],
      currentHashrate: 3,
      currentDifficulty: 110,
    });
    expect(s.x).toStrictEqual([10, 20, 30]);
    expect(s.hashrate).toStrictEqual([1, 2, 3]);
    expect(s.difficulty[0]).toBeCloseTo(100);
    expect(s.difficulty.slice(1)).toStrictEqual([110, 110]);
  });
  it("uses the current difficulty when no retarget falls in the period", () => {
    const s = hashrateSeries({
      hashrates: [{ timestamp: 10, avgHashrate: 1 }],
      difficulty: [],
      currentHashrate: 1,
      currentDifficulty: 148,
    });
    expect(s.difficulty).toStrictEqual([148]);
  });
});

describe("pool layers", () => {
  const week = (timestamp: number, poolName: string, share: number) => ({
    timestamp,
    poolName,
    share,
    avgHashrate: 0,
  });
  it("stacks the coloured pools, biggest at the bottom, and every other pool as one layer", () => {
    const weeks = [
      week(2, "AntPool", 0.5),
      week(2, "Luxor", 0.3),
      week(2, "Unknown", 0.1),
      week(2, "F2Pool", 0.1),
      week(1, "AntPool", 0.4),
      week(1, "F2Pool", 0.6),
    ];
    const { x, layers } = poolLayers(weeks);
    expect(x).toStrictEqual([1, 2]);
    expect(layers.map((l) => l.name)).toStrictEqual(["AntPool", "F2Pool", OTHER_POOLS]);
    expect(layers[0]?.values).toStrictEqual([0.4, 0.5]);
    expect(layers[2]?.values[1]).toBeCloseTo(0.4);
    // A pool's colour is its own, whatever its rank: AntPool is 2 and F2Pool 3 here and everywhere.
    expect(layers.map((l) => l.pool)).toStrictEqual([2, 3, 0]);
  });
  it("leaves out the grey layer when every pool has a colour", () => {
    const { layers } = poolLayers([week(1, "ViaBTC", 0.7), week(1, "Foundry USA", 0.3)]);
    expect(layers.map((l) => [l.name, l.pool])).toStrictEqual([
      ["ViaBTC", 4],
      ["Foundry USA", 1],
    ]);
  });
});

describe("figures", () => {
  const ranking: Ranking = {
    pools: [],
    blockCount: 10,
    lastEstimatedHashrate: 1,
    lastEstimatedHashrate3d: 2,
    lastEstimatedHashrate1w: 3,
  };
  it("takes the network's hashrate over the same stretch as the shares", () => {
    expect(networkHashrate(parsePeriod("24h"), ranking, undefined)).toBe(1);
    expect(networkHashrate(parsePeriod("1w"), ranking, undefined)).toBe(3);
    const h = {
      hashrates: [
        { timestamp: 1, avgHashrate: 4 },
        { timestamp: 2, avgHashrate: 6 },
      ],
      difficulty: [],
      currentHashrate: 6,
      currentDifficulty: 1,
    };
    expect(networkHashrate(parsePeriod("1y"), ranking, h)).toBe(5);
  });
  it("splits a reward into subsidy and fees", () => {
    expect(
      rewardFigures({
        startBlock: 1,
        endBlock: 4,
        totalReward: "1250001000",
        totalFee: "1000",
        totalTx: "40",
      }),
    ).toStrictEqual({
      blocks: 4,
      reward: 1_250_001_000,
      fees: 1000,
      subsidy: 1_250_000_000,
      txs: 40,
      feeShare: 1000 / 1_250_001_000,
      perBlock: 312_500_250,
    });
  });
});

describe("coverage", () => {
  const year = parsePeriod("1y");
  it("says nothing when the data covers the period", () => {
    expect(coverageNote(undefined, year)).toBeUndefined();
    expect(coverageNote({ complete: true, fromHeight: 0, fromTime: 0 }, year)).toBeUndefined();
  });
  it("says where the data starts when it does not", () => {
    const text = coverageNote(
      { complete: false, fromHeight: 968_000, fromTime: Date.UTC(2026, 8, 22) / 1000 },
      year,
    );
    expect(text).toMatch(/^Data from 22 Sep 2026 onward\./);
    expect(text).toContain("the last year is not all here yet");
  });
  it("says when nothing is stored", () => {
    expect(
      coverageNote({ complete: false, fromHeight: undefined, fromTime: undefined }, year),
    ).toBe("No blocks are stored for the last year yet.");
  });
});

describe("networks", () => {
  it("explains each test network and leaves mainnet alone", () => {
    expect(networkNote("mainnet")).toBeUndefined();
    expect(networkNote("signet")).toContain("signers");
    expect(networkNote("testnet4")).toContain("minimum difficulty");
    expect(networkNote("regtest")).toContain("no pools");
  });
});

describe("data-derived links", () => {
  it("allows plain http and https only", () => {
    expect(safeLink("https://www.foundrydigital.com/")).toStrictEqual({
      href: "https://www.foundrydigital.com/",
      host: "foundrydigital.com",
    });
    // oxlint-disable-next-line no-script-url -- hostile data the check must refuse
    expect(safeLink("javascript:alert(1)")).toBeUndefined();
    expect(safeLink("data:text/html,<b>x</b>")).toBeUndefined();
    expect(safeLink("https://user:pass@example.com")).toBeUndefined();
    expect(safeLink("not a url")).toBeUndefined();
    expect(safeLink("")).toBeUndefined();
  });
});

describe("reply checks", () => {
  it("accepts the ranking's shape and refuses others", () => {
    expect(
      isRanking({
        pools: [
          {
            poolId: 0,
            name: "Unknown",
            link: "",
            blockCount: 1,
            rank: 1,
            emptyBlocks: 0,
            slug: "unknown",
          },
        ],
        blockCount: 1,
        lastEstimatedHashrate: null,
        lastEstimatedHashrate3d: null,
        lastEstimatedHashrate1w: null,
      }),
    ).toBe(true);
    expect(isRanking({ pools: [{ name: 1 }], blockCount: 1 })).toBe(false);
    expect(isRanking(null)).toBe(false);
  });
  it("refuses a pool whose totals are not whole numbers", () => {
    const stats = {
      pool: { id: 1, name: "A", link: "", addresses: [], regexes: [], slug: "a" },
      blockCount: { all: 1, "24h": 1, "1w": 1 },
      blockShare: { all: 1, "24h": 1, "1w": 1 },
      estimatedHashrate: null,
      totalReward: "625000000",
    };
    expect(isPoolStats(stats)).toBe(true);
    expect(isPoolStats({ ...stats, totalReward: "1e9" })).toBe(false);
  });
});

describe("ordering and gaps", () => {
  it("orders block points by time, which can run backwards across heights", () => {
    const pts = [
      { avgHeight: 1, timestamp: 100 },
      { avgHeight: 2, timestamp: 300 },
      { avgHeight: 3, timestamp: 200 },
    ];
    expect(byTime(pts)?.map((p) => p.avgHeight)).toStrictEqual([1, 3, 2]);
    expect(byTime(undefined)).toBeUndefined();
  });
  it("fills a pool's weeks without a block as zero, inventing none outside its stored weeks", () => {
    const W = 7 * DAY;
    const week = (timestamp: number, share: number) => ({
      timestamp,
      share,
      avgHashrate: share * 1e21,
      poolName: "A",
    });
    const filled = fillWeeks([week(3 * W, 0.2), week(W, 0.3)]);
    expect(filled.map((w) => [w.timestamp, w.share])).toStrictEqual([
      [W, 0.3],
      [2 * W, 0],
      [3 * W, 0.2],
    ]);
    expect(fillWeeks([])).toStrictEqual([]);
  });
});
