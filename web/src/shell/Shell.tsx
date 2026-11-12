import { type RouteSectionProps, useLocation, useNavigate } from "@solidjs/router";
import {
  createEffect,
  createResource,
  createSignal,
  ErrorBoundary,
  on,
  onCleanup,
  Show,
  Suspense,
} from "solid-js";
import { ErrorState, Loading } from "../components/States";
import { CHUNK_ATTEMPTS, chunkDelay, isChunkError, retryStyles } from "../lib/chunks";
import { fetchConfig } from "../lib/config";
import { NETWORK_NAMES, type Network, onNetwork, stripNetwork } from "../lib/network";
import { useNetwork } from "../lib/page";
import { Footer } from "./Footer";
import { Header } from "./Header";
import "./Shell.css";

/** The frame around every page: band, header, main and footer. The router's root component. */
export function Shell(props: RouteSectionProps) {
  const network = useNetwork();
  const location = useLocation();
  const navigate = useNavigate();
  const [config] = createResource(fetchConfig);
  // Undefined until the server answers, so the switch keeps its full shape meanwhile.
  const networks = (): readonly Network[] | undefined => {
    if (config.loading) return undefined;
    const enabled = config()?.networks;
    return enabled && enabled.length > 0 ? enabled : [network()];
  };
  let main: HTMLElement | undefined;

  createEffect(() => {
    document.documentElement.dataset["network"] = network();
  });

  // A path on a network this server does not serve (mainnet at the root) moves to one it does.
  createEffect(() => {
    const enabled = config()?.networks;
    const first = enabled?.[0];
    if (first && !enabled.includes(network())) {
      navigate(onNetwork(first, stripNetwork(location.pathname)), { replace: true });
    }
  });

  // Each page gets its own automatic attempts at files that failed to download.
  createEffect(
    on(
      () => location.pathname,
      () => {
        chunkAttempts = 0;
      },
    ),
  );

  // After a navigation, focus moves to the new page so screen readers start there.
  createEffect(
    on(
      () => location.pathname,
      () => main?.focus({ preventScroll: true }),
      { defer: true },
    ),
  );

  return (
    <>
      {/* rel="external" keeps the router out, so the browser moves focus to main itself. */}
      <a class="skip" href="#main" rel="external">
        Skip to content
      </a>
      <Show when={network() !== "mainnet"}>
        <div class="network-band">
          <p class="network-band__text">
            {NETWORK_NAMES[network()]} is a test network. Coins here have no value.
          </p>
        </div>
      </Show>
      <Header networks={networks} />
      <main id="main" class="site-main" tabindex="-1" ref={main}>
        <ErrorBoundary
          fallback={(error, reset) => {
            createEffect(on(() => location.pathname, reset, { defer: true }));
            return (
              <div class="wrap">
                <PageFailure error={error} reset={reset} version={config()?.version} />
              </div>
            );
          }}
        >
          <Suspense
            fallback={
              <div class="wrap">
                <Loading />
              </div>
            }
          >
            {props.children}
          </Suspense>
        </ErrorBoundary>
      </main>
      <Footer version={() => config()?.version} />
    </>
  );
}

/** Automatic attempts made so far at the page's files; the reader's own "Try again" starts afresh. */
let chunkAttempts = 0;

/**
 * A page that could not be shown. When its code or styles failed to download, they are asked for
 * again, waiting longer each time, while the page shows that it is loading; only then does it ask
 * the reader. If the server's version changed since this tab loaded, the files it needs were
 * replaced by a deploy, so it says so and offers a reload instead.
 */
function PageFailure(props: { error: unknown; reset: () => void; version: string | undefined }) {
  const chunk = isChunkError(props.error);
  const [state, setState] = createSignal<"waiting" | "updated" | "failed">(
    chunk && chunkAttempts < CHUNK_ATTEMPTS ? "waiting" : "failed",
  );
  let alive = true;
  onCleanup(() => {
    alive = false;
  });
  const updated = async () => {
    const now = (await fetchConfig())?.version;
    return props.version !== undefined && now !== undefined && now !== props.version;
  };
  const recover = async () => {
    while (chunkAttempts < CHUNK_ATTEMPTS) {
      const wait = chunkDelay(chunkAttempts++);
      // oxlint-disable-next-line no-await-in-loop -- each attempt waits longer than the last
      await new Promise((resolve) => {
        setTimeout(resolve, wait);
      });
      if (!alive) return;
      // oxlint-disable-next-line no-await-in-loop -- asked before each attempt, once the wait is over
      if (await updated()) {
        setState("updated");
        return;
      }
      try {
        // oxlint-disable-next-line no-await-in-loop -- the page is shown only once its styles are in
        await retryStyles();
        if (alive) props.reset();
        return;
      } catch {
        // A stylesheet failed again: wait longer and try once more.
      }
    }
    if (alive) setState("failed");
  };
  if (state() === "waiting") void recover();

  const tryAgain = async () => {
    chunkAttempts = CHUNK_ATTEMPTS;
    setState("waiting");
    if (chunk && (await updated())) {
      setState("updated");
      return;
    }
    // A render error can follow a stylesheet that failed too; either way everything is asked again.
    const styles = await retryStyles().then(
      () => true,
      () => false,
    );
    if (styles || !chunk) props.reset();
    else setState("failed");
  };

  return (
    <Show when={state() !== "waiting"} fallback={<Loading label="Loading this page" />}>
      <Show
        when={state() === "updated"}
        fallback={
          <ErrorState
            title="This page could not be shown"
            error={props.error}
            {...(chunk
              ? {
                  message:
                    "Part of it could not be downloaded. Check your connection and try again. If this keeps happening, reload the page.",
                }
              : {})}
            retry={() => void tryAgain()}
          />
        }
      >
        <ErrorState
          title="This site has been updated"
          message="This tab still runs the version from before, and the page you opened needs files the update replaced. Reload to use the new version."
          action="Reload"
          retry={() => window.location.reload()}
        />
      </Show>
    </Show>
  );
}
