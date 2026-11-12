// Typed HTTP client for the public API at /api (Esplora, per network prefix). docs/FRONTEND.md shows how to use it.
import { useLocation } from "@solidjs/router";
import { createMemo, createResource, onCleanup, type ResourceReturn } from "solid-js";
import type { ChainTip } from "./blocks-topic";
import { apiBase, type Network, networkFromPath, onNetwork } from "./network";

/**
 * A failed request. `status` is the HTTP status, or 0 when no reply arrived. `retryAfter` is the
 * reply's `Retry-After` in seconds, when it sent one.
 */
export class ApiError extends Error {
  readonly status: number;
  readonly retryAfter: number | undefined;
  constructor(status: number, message: string, retryAfter?: number) {
    super(message);
    this.name = "ApiError";
    this.status = status;
    this.retryAfter = retryAfter;
  }
}

export const isNotFound = (e: unknown): boolean => e instanceof ApiError && e.status === 404;

const STATUS_MESSAGES: Record<number, string> = {
  400: "The request was not valid.",
  404: "Not found.",
  413: "The request was too large.",
  422: "The request was not valid.",
  429: "Too many requests. Try again in a moment.",
  503: "The node cannot answer right now. Try again in a moment.",
  504: "The node took too long to answer. Try again in a moment.",
};

/** The message for a failed reply. Our server sends `{"error": "..."}`; anything else gets a generic one. */
export function errorMessage(status: number, body: unknown): string {
  if (typeof body === "object" && body !== null && "error" in body) {
    const text = body.error;
    if (typeof text === "string" && text.trim() !== "") {
      const t = text.trim();
      return `${t[0]?.toUpperCase()}${t.slice(1)}${/[.!?]$/.test(t) ? "" : "."}`;
    }
  }
  return (
    STATUS_MESSAGES[status] ?? (status >= 500 ? "The server could not answer." : `HTTP ${status}.`)
  );
}

/** Whether a reply can be kept for the life of the page: the server marked it as never changing. */
export function isImmutable(cacheControl: string | null): boolean {
  if (!cacheControl) return false;
  const cc = cacheControl.toLowerCase();
  if (/(^|,)\s*(no-store|no-cache|private)\b/.test(cc)) return false;
  const maxAge = /(?:^|,)\s*max-age=(\d+)/.exec(cc);
  return /(^|,)\s*immutable\b/.test(cc) || (maxAge !== null && Number(maxAge[1]) >= 86_400);
}

/** `bytes` is an ArrayBuffer, for binary replies such as a block's transaction records. */
export type ReplyKind = "json" | "text" | "bytes";

// shortcut: a plain insertion-ordered map capped at CACHE_MAX entries; make it LRU if pages re-read old entries often.
const CACHE_MAX = 256;
const cache = new Map<string, unknown>();

function remember(url: string, value: unknown) {
  if (cache.size >= CACHE_MAX) {
    const oldest = cache.keys().next();
    if (!oldest.done) cache.delete(oldest.value);
  }
  cache.set(url, value);
}

/** Empties the response cache. For tests. */
export function clearApiCache(): void {
  cache.clear();
  notBefore.clear();
}

/** A reply with the chain tip it is as of (`X-Bitcoin-Chain-Tip`); undefined when not stamped or cached. */
export interface Stamped<T> {
  value: T;
  tip: ChainTip | undefined;
  /** How far back a history answer reaches (the mining routes); absent when not said. */
  coverage?: Coverage;
}

/**
 * The `X-Data-*` headers of a history answer (docs/API.md "Coverage"): the earliest block it
 * includes (unix seconds), and whether it covers the whole period asked for.
 */
export interface Coverage {
  fromHeight: number | undefined;
  fromTime: number | undefined;
  complete: boolean;
}

/** A reply's coverage, when it states one. */
export function readCoverage(headers: Headers): Coverage | undefined {
  const complete = headers.get("x-data-complete");
  if (complete === null) return undefined;
  const num = (name: string) => {
    const n = Number(headers.get(name) ?? Number.NaN);
    return Number.isSafeInteger(n) && n >= 0 ? n : undefined;
  };
  return {
    fromHeight: num("x-data-from-height"),
    fromTime: num("x-data-from-time"),
    complete: complete === "true",
  };
}

/** The tip a reply's headers name, if they name a whole one. */
export function chainTip(headers: Headers): ChainTip | undefined {
  const hash = headers.get("x-bitcoin-chain-tip") ?? "";
  const height = Number(headers.get("x-bitcoin-chain-tip-height") ?? Number.NaN);
  return /^[0-9a-f]{64}$/.test(hash) && Number.isSafeInteger(height) && height >= 0
    ? { hash, height }
    : undefined;
}

/** `Retry-After` in seconds; the HTTP-date form is not used by our server. */
function readRetryAfter(headers: Headers): number | undefined {
  const s = Number(headers.get("retry-after") ?? Number.NaN);
  return Number.isFinite(s) && s >= 0 ? s : undefined;
}

const ACCEPT: Record<ReplyKind, string> = {
  json: "application/json",
  text: "text/plain",
  bytes: "application/octet-stream",
};

/**
 * When each refused URL may be asked again, from its `Retry-After`. Kept across calls, so a poll
 * that abandons one retry loop and starts another still waits as the server said.
 */
const notBefore = new Map<string, number>();

async function request<T>(url: string, kind: ReplyKind, signal?: AbortSignal): Promise<Stamped<T>> {
  const until = notBefore.get(url) ?? 0;
  if (until > Date.now()) await pause(until - Date.now(), signal);
  notBefore.delete(url);
  let res: Response;
  try {
    res = await fetch(url, {
      headers: { accept: ACCEPT[kind] },
      ...(signal ? { signal } : {}),
    });
  } catch (e) {
    if (signal?.aborted) throw e;
    throw new ApiError(
      0,
      navigator.onLine
        ? "The server cannot be reached. Check your connection."
        : "You are offline. Reconnect, then try again.",
    );
  }
  if (!res.ok) {
    const body: unknown = await res.json().catch(() => null);
    const retryAfter = readRetryAfter(res.headers);
    if ((res.status === 503 || res.status === 429) && retryAfter !== undefined) {
      notBefore.set(url, Date.now() + retryAfter * 1000);
    }
    throw new ApiError(res.status, errorMessage(res.status, body), retryAfter);
  }
  let value: unknown;
  try {
    value =
      kind === "json"
        ? await res.json()
        : kind === "bytes"
          ? await res.arrayBuffer()
          : (await res.text()).trim();
  } catch (e) {
    if (signal?.aborted) throw e;
    throw new ApiError(res.status, "The server sent a reply that could not be read.");
  }
  if (isImmutable(res.headers.get("cache-control"))) remember(url, value);
  // The API's shapes are hand-written in esplora.ts; values are rendered as text, never as markup.
  const coverage = readCoverage(res.headers);
  return { value: value as T, tip: chainTip(res.headers), ...(coverage ? { coverage } : {}) };
}

/** Every request is a GET, safe to repeat, so a busy refusal is retried as it says ({@link withRetry}). */
function cachedOrFetch<T>(url: string, kind: ReplyKind, signal?: AbortSignal): Promise<Stamped<T>> {
  if (cache.has(url)) return Promise.resolve({ value: cache.get(url) as T, tip: undefined });
  return withRetry(() => request<T>(url, kind, signal), signal);
}

/**
 * One API call outside a component. `path` starts with `/` and is relative to the network's API,
 * for example `/block/<hash>`. Rejects with {@link ApiError}, or with the abort reason when aborted.
 */
export function fetchApi<T>(
  network: Network,
  path: string,
  options: { as?: ReplyKind; signal?: AbortSignal } = {},
): Promise<T> {
  return cachedOrFetch<T>(`${apiBase(network)}${path}`, options.as ?? "json", options.signal).then(
    (r) => r.value,
  );
}

/** {@link fetchApi} with the chain tip the reply is as of, to compare it with what the page knows. */
export function fetchStamped<T>(
  network: Network,
  path: string,
  options: { signal?: AbortSignal } = {},
): Promise<Stamped<T>> {
  return cachedOrFetch<T>(`${apiBase(network)}${path}`, "json", options.signal);
}

/** {@link fetchApi} for the app's own API under the network's `/app` (docs/FRONTEND.md). */
export function fetchApp<T>(
  network: Network,
  path: string,
  options: { as?: ReplyKind; signal?: AbortSignal } = {},
): Promise<T> {
  const url = `${onNetwork(network, "")}/app${path}`;
  return cachedOrFetch<T>(url, options.as ?? "json", options.signal).then((r) => r.value);
}

/** Retries after a refusal that says to (503, 429), this many times before giving up. */
export const RETRY_ATTEMPTS = 6;
const RETRY_MAX_MS = 30_000;
/** Refusals a retry loop already gave up on, so a loop around another never retries them again. */
const exhausted = new WeakSet();

/**
 * How long to wait before retry `attempt` (0-based) of a failed call, or undefined when it should
 * not be retried: only 503 and 429 are (not the permanent 503s), never sooner than their `Retry-After`, backing off
 * exponentially from 1 s to 30 s, with up to a quarter more at random so clients spread out.
 */
export function retryDelay(
  error: unknown,
  attempt: number,
  random: () => number = Math.random,
): number | undefined {
  if (
    !(error instanceof ApiError) ||
    (error.status !== 503 && error.status !== 429) ||
    exhausted.has(error) ||
    // Every 503 carries Retry-After, but these refusals are about the address and the server's
    // setup, which a retry cannot change (docs/API.md, "Error bodies" and "Mining").
    /too large|index is disabled|needs a block table/i.test(error.message)
  ) {
    return undefined;
  }
  if (attempt >= RETRY_ATTEMPTS) return undefined;
  const told = (error.retryAfter ?? 1) * 1000;
  const backoff = Math.min(RETRY_MAX_MS, 1000 * 2 ** attempt);
  return Math.round(Math.max(told, backoff) * (1 + random() / 4));
}

const abortReason = (signal: AbortSignal): Error => {
  const reason: unknown = signal.reason;
  return reason instanceof Error ? reason : new DOMException("Aborted", "AbortError");
};

/**
 * Runs `call` until it succeeds, retrying refusals as {@link retryDelay} says. Rejects with the
 * last error, or with the abort reason when `signal` aborts while waiting. Every fetch in this
 * file already runs inside it; only wrap a call that is safe to repeat.
 */
export async function withRetry<T>(call: () => Promise<T>, signal?: AbortSignal): Promise<T> {
  for (let attempt = 0; ; attempt++) {
    try {
      // oxlint-disable-next-line no-await-in-loop -- each attempt waits for the one before
      return await call();
    } catch (e) {
      if (signal?.aborted) throw abortReason(signal);
      const wait = retryDelay(e, attempt);
      if (wait === undefined) {
        if (e instanceof ApiError) exhausted.add(e);
        throw e;
      }
      // oxlint-disable-next-line no-await-in-loop -- the wait is the point
      await pause(wait, signal);
    }
  }
}

/** Waits `ms`, or rejects with the abort reason as soon as `signal` aborts. */
function pause(ms: number, signal: AbortSignal | undefined): Promise<void> {
  if (signal?.aborted) return Promise.reject(abortReason(signal));
  return new Promise<void>((resolve, reject) => {
    const timer = setTimeout(resolve, ms);
    signal?.addEventListener(
      "abort",
      () => {
        clearTimeout(timer);
        reject(abortReason(signal));
      },
      { once: true },
    );
  });
}

/**
 * A Solid resource for one API path on the current page's network. Return a falsy path to wait.
 * Refetches when the path or network changes, cancels the request it replaces and the one in
 * flight when the page goes away, answers at once from the cache, and retries busy refusals.
 */
export function createApi<T>(
  path: () => string | false | null | undefined,
  options: { as?: ReplyKind; stamped: true },
): ResourceReturn<Stamped<T>>;
export function createApi<T>(
  path: () => string | false | null | undefined,
  options?: { as?: ReplyKind },
): ResourceReturn<T>;
/** With `stamped: true` the value comes with the chain tip it is as of, to compare across requests. */
export function createApi<T>(
  path: () => string | false | null | undefined,
  options: { as?: ReplyKind; stamped?: boolean } = {},
): ResourceReturn<T | Stamped<T>> {
  const location = useLocation();
  const network = createMemo(() => networkFromPath(location.pathname));
  let controller: AbortController | undefined;
  onCleanup(() => controller?.abort());
  return createResource(
    () => {
      const p = path();
      // oxlint-disable-next-line typescript/strict-boolean-expressions -- every falsy path means "no request"
      return p ? `${apiBase(network())}${p}` : false;
    },
    (url): T | Stamped<T> | Promise<T | Stamped<T>> => {
      controller?.abort();
      controller = undefined;
      const unwrap = (r: Stamped<T>) => (options.stamped === true ? r : r.value);
      if (cache.has(url)) return unwrap({ value: cache.get(url) as T, tip: undefined });
      const c = new AbortController();
      controller = c;
      // The resource stays loading while a busy refusal waits to be retried.
      return withRetry(() => request<T>(url, options.as ?? "json", c.signal), c.signal).then(
        unwrap,
      );
    },
  );
}
