// Hooks every page uses: the current network and the document title.
import { useLocation } from "@solidjs/router";
import { type Accessor, createEffect, createMemo } from "solid-js";
import { NETWORK_NAMES, type Network, networkFromPath } from "./network";

/** The network of the page on screen. */
export function useNetwork(): Accessor<Network> {
  const location = useLocation();
  return createMemo(() => networkFromPath(location.pathname));
}

/** Sets the document title; test networks get their name in front so tabs are never ambiguous. */
export function usePageTitle(title: () => string): void {
  const network = useNetwork();
  createEffect(
    () =>
      `${network() === "mainnet" ? "" : `${NETWORK_NAMES[network()]}: `}${title()} | rbitcoin.space`,
    (text) => {
      document.title = text;
    },
  );
}
