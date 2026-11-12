// What a search query is, from its form alone. Pure: nothing here fetches, and a pasted link is
// only read for its path, never followed.
import { type Recognised, recognise } from "../lib/address";

/** Longer input is not anything search can find; the field holds no more than this. */
export const MAX_QUERY = 512;

/** What a link's path or a `txid:vout` says a 64-hex id is. */
export type HashKind = "block" | "tx";

export type Query =
  | { kind: "height"; height: number }
  | { kind: "hash"; hash: string; as: HashKind | null }
  | { kind: "address"; address: Recognised };

type Hint = HashKind | "address" | null;

const U32_MAX = 4_294_967_295;

/** Path words that say what the next segment is, on this and other explorers. */
const PATH_WORDS: Record<string, Hint> = {
  tx: "tx",
  transaction: "tx",
  transactions: "tx",
  block: "block",
  blocks: "block",
  "block-height": "block",
  address: "address",
  addresses: "address",
};

// Zero-width characters ride along with copied text; quotes, brackets and a sentence's final
// punctuation are not part of any identifier. (Escapes in \u{} form: oxfmt unescapes \uXXXX.)
const INVISIBLE = /[\u{200B}-\u{200D}\u{2060}\u{FEFF}]/gu;
const WRAPPING = /^[\s"'`<([{\u{2018}\u{201C}]+|[\s"'`>)\]}\u{2019}\u{201D}.,;]+$/gu;

/** Classifies a query, or null when it has no form search can find. */
export function classify(input: string): Query | null {
  if (input.length > MAX_QUERY) return null;
  const q = input.replaceAll(INVISIBLE, "").replaceAll(WRAPPING, "");
  const uri = /^bitcoin:(?:\/\/)?([^?#]*)/i.exec(q);
  if (uri) return plain(decode(uri[1] ?? ""), "address");
  if (q.includes("/")) return fromLink(q);
  return plain(q, null);
}

function decode(segment: string): string {
  try {
    return decodeURIComponent(segment);
  } catch {
    return "";
  }
}

/**
 * The identifier in a pasted link's path: the first segment that fits the nearest path word
 * before it (`/tx/<id>`, `/bitcoin/transaction/<id>`, `/explorer/blocks/btc/<height>`). With no
 * path word, a hash or address as the last segment. The host is ignored, so any explorer's link works.
 */
function fromLink(q: string): Query | null {
  let url: URL;
  try {
    // The base only lets a bare path or `host/path` parse; it is never fetched.
    url = new URL(q, "https://search.invalid");
  } catch {
    return null;
  }
  if (url.protocol !== "https:" && url.protocol !== "http:") return null;
  const segments = url.pathname
    .split("/")
    .filter(Boolean)
    .map((s) => decode(s));
  let hint: Hint = null;
  for (const segment of segments) {
    const word = PATH_WORDS[segment.toLowerCase()];
    if (word) hint = word;
    else if (hint) {
      const found = plain(segment, hint);
      if (found) return found;
    }
  }
  if (hint) return null;
  const last = plain(segments.at(-1) ?? "", null);
  return last?.kind === "height" ? null : last;
}

/** A bare identifier, limited to what `hint` allows. */
function plain(q: string, hint: Hint): Query | null {
  // Hashes and addresses never hold whitespace; a copied one may be grouped or wrapped.
  const compact = q.replaceAll(/\s+/g, "");
  if (hint !== "address") {
    const hex = /^(?:0x)?([0-9a-f]{64})(?::\d{1,10})?$/i.exec(compact);
    const hash = hex?.[1]?.toLowerCase();
    const outpoint = compact.includes(":");
    if (hash !== undefined && !(outpoint && hint === "block")) {
      return { kind: "hash", hash, as: outpoint ? "tx" : hint };
    }
  }
  if (hint === null || hint === "block") {
    const height = parseHeight(q);
    if (height !== null) return { kind: "height", height };
  }
  if (hint === null || hint === "address") {
    const address = recognise(compact);
    if (address) return { kind: "address", address };
  }
  return null;
}

/**
 * A block height as people write it: `975411`, `975,411`, `975 411`, `975.411`, `975_411`,
 * `#975411` or `Block 975 411`. Groups of three after one consistent separator.
 */
function parseHeight(q: string): number | null {
  const m = /^(?:block\s*)?#?\s*(\d{1,10}|\d{1,3}([,.'_\s])\d{3}(?:\2\d{3}){0,2})$/i.exec(q);
  if (!m?.[1]) return null;
  const height = Number(m[1].replaceAll(/\D/g, ""));
  return height <= U32_MAX ? height : null;
}
