import { type RouteSectionProps, useLocation, useNavigate } from "@solidjs/router";
import { createEffect, createResource, ErrorBoundary, on, Show, Suspense } from "solid-js";
import { ErrorState, Loading } from "../components/States";
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
                <ErrorState title="This page could not be shown" error={error} retry={reset} />
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
