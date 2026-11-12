import type { Accessor } from "solid-js";
import type { LiveStatus } from "../../lib/live";
import { FALLBACK_POLL_MS } from "./feeds";

/** What the indicator shows: the socket's state, or that the node behind a live socket is silent. */
export type Shown = LiveStatus | "stale";

const LABELS: Record<Shown, string> = {
  live: "Live",
  connecting: "Connecting",
  reconnecting: "Reconnecting",
  offline: "Offline",
  stale: "Node not answering",
};

/** What the reader should know when not live: what still updates, and what may be old. */
function detail(shown: Shown): string {
  const every = `Blocks refresh every ${FALLBACK_POLL_MS / 1000} s meanwhile.`;
  if (shown === "reconnecting") return `Live updates paused. ${every}`;
  if (shown === "stale") return "Figures may be out of date.";
  if (shown === "offline") {
    return navigator.onLine ? `Reload the page for live updates. ${every}` : "You are offline.";
  }
  return "";
}

/**
 * The live connection's state, as an icon and a word: a pulsing dot when live, a dashed ring while
 * connecting or when the node is silent, a crossed ring when offline. `stale` says the server is
 * connected but its node is not answering, so what is on screen may be old.
 */
export function LiveIndicator(props: { status: Accessor<LiveStatus>; stale: Accessor<boolean> }) {
  const shown = (): Shown =>
    props.status() === "live" && props.stale() ? "stale" : props.status();
  return (
    <p class={`home-live home-live--${shown()}`} role="status">
      <svg class="home-live__icon" viewBox="0 0 12 12" aria-hidden="true">
        <circle class="home-live__dot" cx="6" cy="6" r="4" />
        <path class="home-live__cross" d="M3.5 3.5l5 5M8.5 3.5l-5 5" />
      </svg>
      <span class="home-live__label">{LABELS[shown()]}</span>
      <span class="home-live__detail">{detail(shown())}</span>
    </p>
  );
}
