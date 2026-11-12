import { createSignal, onCleanup, Show } from "solid-js";
import "./CopyButton.css";

const CONFIRM_MS = 1400;
const FAILED_MS = 3000;

/**
 * Copies text with the synchronous copy command, from a hidden read-only textarea. Plain HTTP
 * leaves only this way, since the async clipboard exists only in a secure context. It counts as
 * the reader's gesture only inside the click's own task. Says whether the browser copied.
 */
export function copyByCommand(value: string): boolean {
  const focused = document.activeElement;
  const { scrollX, scrollY } = window;
  const area = document.createElement("textarea");
  area.className = "copy__scratch";
  // Read-only, so a phone shows no keyboard; the value goes in as text.
  area.readOnly = true;
  area.value = value;
  area.setAttribute("aria-hidden", "true");
  area.tabIndex = -1;
  document.body.append(area);
  area.select();
  // iOS Safari copies only a selection range set on the element itself.
  area.setSelectionRange(0, value.length);
  let copied = false;
  try {
    // oxlint-disable-next-line typescript/no-deprecated -- the only way to copy outside a secure context
    copied = document.execCommand("copy");
  } catch {
    // Some browsers throw rather than answer false.
  }
  area.remove();
  if (focused instanceof HTMLElement) focused.focus({ preventScroll: true });
  window.scrollTo(scrollX, scrollY);
  return copied;
}

/**
 * Copies text and says whether it landed: the async clipboard where the page has it, the copy
 * command where it has not (plain HTTP) or where it refused.
 */
export async function copyText(value: string): Promise<boolean> {
  // Undefined outside a secure context, whatever the types say.
  const clipboard = navigator.clipboard as Clipboard | undefined;
  // Runs before the first await, so still inside the click's task.
  if (!clipboard) return copyByCommand(value);
  try {
    await clipboard.writeText(value);
    return true;
  } catch {
    return copyByCommand(value);
  }
}

/** Copies a value. Says "Copied" only when it was, and says so when it could not be. */
export function CopyButton(props: { value: string; label?: string }) {
  const [state, setState] = createSignal<"idle" | "copied" | "failed">("idle");
  let timer: ReturnType<typeof setTimeout> | undefined;
  onCleanup(() => clearTimeout(timer));

  const copy = async () => {
    const copied = await copyText(props.value);
    setState(copied ? "copied" : "failed");
    clearTimeout(timer);
    timer = setTimeout(() => setState("idle"), copied ? CONFIRM_MS : FAILED_MS);
  };

  return (
    <button
      class="copy"
      type="button"
      onClick={() => void copy()}
      data-copied={state() === "copied" ? "" : undefined}
      data-failed={state() === "failed" ? "" : undefined}
      title={props.label ?? "Copy"}
    >
      <Show
        when={state() === "failed"}
        fallback={
          <svg viewBox="0 0 16 16" aria-hidden="true">
            <rect x="5" y="5" width="8.5" height="8.5" rx="1.5" />
            <path d="M3 10.5V3.5A1 1 0 0 1 4 2.5h6.5" />
          </svg>
        }
      >
        <svg viewBox="0 0 16 16" aria-hidden="true">
          <path d="M4 4l8 8M12 4l-8 8" />
        </svg>
        <span class="copy__note" aria-hidden="true">
          Not copied
        </span>
      </Show>
      <span class="visually-hidden" aria-live="polite">
        {state() === "copied"
          ? "Copied"
          : state() === "failed"
            ? "Could not copy"
            : (props.label ?? "Copy")}
      </span>
    </button>
  );
}
