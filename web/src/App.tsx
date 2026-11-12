import { createSignal, onCleanup, onMount } from "solid-js";
import { fetchHealth, type Health } from "./health";

const POLL_MS = 5000;

export function App() {
  const [health, setHealth] = createSignal<Health>();
  const [checkedAt, setCheckedAt] = createSignal<string>();

  const check = async () => {
    setHealth(await fetchHealth());
    setCheckedAt(new Date().toLocaleTimeString());
  };

  onMount(() => {
    void check();
    const timer = setInterval(() => void check(), POLL_MS);
    onCleanup(() => clearInterval(timer));
  });

  const state = () => {
    const h = health();
    return h === undefined ? "pending" : h.ok ? "up" : "down";
  };

  const message = () => {
    const h = health();
    if (h === undefined) return "Checking the server…";
    return h.ok ? `Server is up, version ${h.version}.` : `Server is down: ${h.reason}.`;
  };

  return (
    <main class="page">
      <header class="brand">
        <img class="brand-mark" src="/icon.svg" alt="" width="56" height="56" />
        <div>
          <h1>mempool.based</h1>
          <p class="tagline">A block explorer for rbitcoin.</p>
        </div>
      </header>

      <section class="health" data-state={state()} aria-live="polite">
        <span class="health-tile" aria-hidden="true" />
        <p>
          {message()} <span class="checked">{checkedAt() && `Checked at ${checkedAt()}.`}</span>
        </p>
      </section>
    </main>
  );
}
