// Code and styles loaded on demand, and what to do when they fail to download (docs/FRONTEND.md,
// "When a page's files fail to load").

/** Stylesheets whose download failed. Vite never asks for one again by itself. */
const failedStyles = new Set<HTMLLinkElement>();

/** Notes every stylesheet that fails to load from now on. Call once, before the first page loads. */
export function watchStyles(): void {
  // Capture: a load error does not bubble.
  document.addEventListener(
    "error",
    (e) => {
      if (e.target instanceof HTMLLinkElement && e.target.rel === "stylesheet") {
        failedStyles.add(e.target);
      }
    },
    true,
  );
}

/**
 * Asks again for every stylesheet that failed, each through a fresh `<link>` in place of the old.
 * Waits for every one to settle, so none is still loading when it answers. Resolves when all have
 * loaded; rejects when any failed again (each such one is noted again).
 */
export async function retryStyles(): Promise<void> {
  const links = [...failedStyles];
  failedStyles.clear();
  const settled = await Promise.allSettled(
    links.map(
      (old) =>
        new Promise<void>((resolve, reject) => {
          const link = old.cloneNode() as HTMLLinkElement;
          link.addEventListener("load", () => resolve(), { once: true });
          link.addEventListener("error", () => reject(new Error(`Could not load ${link.href}`)), {
            once: true,
          });
          old.replaceWith(link);
        }),
    ),
  );
  const failed = settled.find((r) => r.status === "rejected");
  if (failed) throw failed.reason;
}

/** Whether an error is a page's code or styles failing to download, as each browser words it. */
export function isChunkError(e: unknown): boolean {
  return (
    e instanceof Error &&
    /dynamically imported module|importing a module script failed|unable to preload css/i.test(
      e.message,
    )
  );
}

/** Automatic attempts before the reader is asked, and the wait before each: 1, 2 and 4 s. */
export const CHUNK_ATTEMPTS = 3;
export const chunkDelay = (attempt: number): number => 1000 * 2 ** attempt;
