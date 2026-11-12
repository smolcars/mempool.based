import { useLocation } from "@solidjs/router";
import { type Accessor, For, Show } from "solid-js";
import { NETWORK_NAMES, type Network, onNetwork, stripNetwork } from "../lib/network";
import { useNetwork } from "../lib/page";
import "./NetworkSwitch.css";

/**
 * The current network, and a menu of links to the same page on the others. Links rather than a
 * select, so arrow keys never navigate and each choice can open in a new tab.
 */
export function NetworkSwitch(props: { networks: Accessor<readonly Network[] | undefined> }) {
  const network = useNetwork();
  const location = useLocation();
  let menu: HTMLDivElement | undefined;
  // While the list is loading, offer the current network only, in the button's final shape.
  const listed = () => props.networks() ?? [network()];
  const switchable = () => props.networks() === undefined || listed().length > 1;
  const hrefOn = (n: Network) =>
    onNetwork(n, stripNetwork(location.pathname)) + location.search + location.hash;

  return (
    <div class="network">
      <Show
        when={switchable()}
        fallback={
          <span class="network__button network__button--static">
            <span class="network__dot" aria-hidden="true" />
            {NETWORK_NAMES[network()]}
          </span>
        }
      >
        <button
          class="network__button"
          type="button"
          popovertarget="network-menu"
          aria-label={`Network: ${NETWORK_NAMES[network()]}. Change network`}
        >
          <span class="network__dot" aria-hidden="true" />
          {NETWORK_NAMES[network()]}
          <svg class="network__chevron" viewBox="0 0 10 6" aria-hidden="true">
            <path d="M1 1l4 4 4-4" />
          </svg>
        </button>
        <div id="network-menu" class="network__menu" popover ref={menu}>
          <ul>
            <For each={listed()}>
              {(n) => (
                <li>
                  <a
                    class="network__option"
                    href={hrefOn(n)}
                    data-network={n}
                    aria-current={n === network() ? "true" : undefined}
                    onClick={() => menu?.hidePopover()}
                  >
                    <span class="network__dot" aria-hidden="true" />
                    {NETWORK_NAMES[n]}
                  </a>
                </li>
              )}
            </For>
          </ul>
        </div>
      </Show>
    </div>
  );
}
