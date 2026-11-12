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

export type ReplyKind = "json" | "text";

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
}

/** A reply with the chain tip it is as of (`X-Bitcoin-Chain-Tip`); undefined when not stamped or cached. */
export interface Stamped<T> {
  value: T;
  tip: ChainTip | undefined;
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

async function request<T>(url: string, kind: ReplyKind, signal?: AbortSignal): Promise<Stamped<T>> {
  let res: Response;
  try {
    res = await fetch(url, {
      headers: { accept: kind === "json" ? "application/json" : "text/plain" },
      ...(signal ? { signal } : {}),
    });
  } catch (e) {
    if (signal?.aborted) throw e;
    throw new ApiError(0, "The server cannot be reached. Check your connection.");
  }
  if (!res.ok) {
    const body: unknown = await res.json().catch(() => null);
    throw new ApiError(res.status, errorMessage(res.status, body), readRetryAfter(res.headers));
  }
  let value: unknown;
  try {
    value = kind === "json" ? await res.json() : (await res.text()).trim();
  } catch (e) {
    if (signal?.aborted) throw e;
    throw new ApiError(res.status, "The server sent a reply that could not be read.");
  }
  if (isImmutable(res.headers.get("cache-control"))) remember(url, value);
  // The API's shapes are hand-written in esplora.ts; values are rendered as text, never as markup.
  return { value: value as T, tip: chainTip(res.headers) };
}

function cachedOrFetch<T>(url: string, kind: ReplyKind, signal?: AbortSignal): Promise<Stamped<T>> {
  if (cache.has(url)) return Promise.resolve({ value: cache.get(url) as T, tip: undefined });
  return request<T>(url, kind, signal);
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
  options: { signal?: AbortSignal } = {},
): Promise<T> {
  return cachedOrFetch<T>(`${onNetwork(network, "")}/app${path}`, "json", options.signal).then(
    (r) => r.value,
  );
}

/** Retries after a refusal that says to (503, 429), this many times before giving up. */
export const RETRY_ATTEMPTS = 6;
const RETRY_MAX_MS = 30_000;

/**
 * How long to wait before retry `attempt` (0-based) of a failed call, or undefined when it should
 * not be retried: only 503 and 429 are, never sooner than their `Retry-After`, backing off
 * exponentially from 1 s to 30 s, with up to a quarter more at random so clients spread out.
 */
export function retryDelay(
  error: unknown,
  attempt: number,
  random: () => number = Math.random,
): number | undefined {
  if (!(error instanceof ApiError) || (error.status !== 503 && error.status !== 429)) {
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
 * last error, or with the abort reason when `signal` aborts while waiting.
 */
export async function withRetry<T>(call: () => Promise<T>, signal?: AbortSignal): Promise<T> {
  for (let attempt = 0; ; attempt++) {
    try {
      // oxlint-disable-next-line no-await-in-loop -- each attempt waits for the one before
      return await call();
    } catch (e) {
      if (signal?.aborted) throw abortReason(signal);
      const wait = retryDelay(e, attempt);
      if (wait === undefined) throw e;
      // oxlint-disable-next-line no-await-in-loop -- the wait is the point
      await new Promise<void>((resolve, reject) => {
        const timer = setTimeout(resolve, wait);
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
  }
}

/**
 * A Solid resource for one API path on the current page's network. Return a falsy path to wait.
 * Refetches when the path or network changes, cancels the request it replaces and the one in
 * flight when the page goes away, and answers at once from the cache.
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
      controller = new AbortController();
      return request<T>(url, options.as ?? "json", controller.signal).then(unwrap);
    },
  );
}
