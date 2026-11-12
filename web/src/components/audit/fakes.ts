// An audit for tests: the example in docs/FRONTEND.md "Block audits".
import type { AuditTotals } from "./model";

export const AUDIT: AuditTotals = {
  match_rate: 99.87,
  expected: { txs: 3100, vsize: 998_000, weight: 3_991_200, fees: 1_500_000 },
  actual: { txs: 3150, vsize: 997_500, weight: 3_990_000, fees: 1_498_000 },
  matched: { txs: 2900, vsize: 950_000 },
  missing: { txs: 3, vsize: 1200 },
  displaced: { txs: 40, vsize: 9000 },
  fresh: { txs: 150, vsize: 35_000 },
  conflicted: { txs: 7, vsize: 2000 },
  added: { txs: 60, vsize: 9500 },
  unseen: { txs: 1, vsize: 200 },
};
