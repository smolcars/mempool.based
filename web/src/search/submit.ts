// The search field's hook: where a query leads. Owned by task W6, which replaces this.
import { paths } from "../lib/network";

/**
 * The network-free page path a query leads to, or null when it cannot be anything we show.
 * shortcut: guesses from the shape alone (a 64-hex query is always a transaction); W6 asks the API.
 */
export function searchTarget(query: string): string | null {
  const q = query.trim();
  if (/^\d{1,9}$/.test(q)) return paths.block(Number(q));
  if (/^[0-9a-fA-F]{64}$/.test(q)) return paths.tx(q.toLowerCase());
  if (/^[a-zA-Z0-9]{14,90}$/.test(q)) return paths.address(q);
  return null;
}
