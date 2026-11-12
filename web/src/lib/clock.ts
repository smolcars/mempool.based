// One shared clock for relative times, ticking only while something on screen reads it.
import { type Accessor, createSignal, onCleanup } from "solid-js";

const nowSeconds = () => Date.now() / 1000;
// Written by the first reader as it mounts, so the clock is current from its first frame.
const [now, setNow] = createSignal(nowSeconds(), { ownedWrite: true });
let readers = 0;
let timer: ReturnType<typeof setInterval> | undefined;

/** Current unix time in seconds, updated every second while the calling component is mounted. */
export function useNow(): Accessor<number> {
  if (readers++ === 0) {
    setNow(nowSeconds());
    timer = setInterval(() => setNow(nowSeconds()), 1000);
  }
  onCleanup(() => {
    if (--readers === 0) clearInterval(timer);
  });
  return now;
}
