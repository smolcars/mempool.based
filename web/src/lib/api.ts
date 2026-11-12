// Typed HTTP client for the public API at /api (Esplora, per network prefix). docs/FRONTEND.md shows how to use it.
import { useLocation } from "@solidjs/router";
import { createMemo, createResource, onCleanup, type ResourceReturn } from "solid-js";
import { apiBase, type Network, networkFromPath } from "./network";

/** A failed request. `status` is the HTTP status, or 0 when no reply arrived. */
export class ApiError extends Error {
  readonly status: number;
  constructor(status: number, message: string) {
    super(message);
    this.name = "ApiError";
    this.status = status;
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

async function request<T>(url: string, kind: ReplyKind, signal?: AbortSignal): Promise<T> {
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
    throw new ApiError(res.status, errorMessage(res.status, body));
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
  return value as T;
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
  const url = `${apiBase(network)}${path}`;
  if (cache.has(url)) return Promise.resolve(cache.get(url) as T);
  return request<T>(url, options.as ?? "json", options.signal);
}

/**
 * A Solid resource for one API path on the current page's network. Return a falsy path to wait.
 * Refetches when the path or network changes, cancels the request it replaces and the one in
 * flight when the page goes away, and answers at once from the cache.
 */
export function createApi<T>(
  path: () => string | false | null | undefined,
  options: { as?: ReplyKind } = {},
): ResourceReturn<T> {
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
    (url): T | Promise<T> => {
      controller?.abort();
      controller = undefined;
      if (cache.has(url)) return cache.get(url) as T;
      controller = new AbortController();
      return request<T>(url, options.as ?? "json", controller.signal);
    },
  );
}
