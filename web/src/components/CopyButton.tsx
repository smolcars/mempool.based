import { createSignal, onCleanup } from "solid-js";
import "./CopyButton.css";

const CONFIRM_MS = 1400;

/** Copies a value. Confirms for 1.4 s even when the clipboard is blocked, so the press is never silent. */
export function CopyButton(props: { value: string; label?: string }) {
  const [copied, setCopied] = createSignal(false);
  let timer: ReturnType<typeof setTimeout> | undefined;
  onCleanup(() => clearTimeout(timer));

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(props.value);
    } catch {
      // Clipboard blocked (insecure context or permission): the state below still confirms the press.
    }
    setCopied(true);
    clearTimeout(timer);
    timer = setTimeout(() => setCopied(false), CONFIRM_MS);
  };

  return (
    <button
      class="copy"
      type="button"
      onClick={copy}
      data-copied={copied() ? "" : undefined}
      title={props.label ?? "Copy"}
    >
      <svg viewBox="0 0 16 16" aria-hidden="true">
        <rect x="5" y="5" width="8.5" height="8.5" rx="1.5" />
        <path d="M3 10.5V3.5A1 1 0 0 1 4 2.5h6.5" />
      </svg>
      <span class="visually-hidden" aria-live="polite">
        {copied() ? "Copied" : (props.label ?? "Copy")}
      </span>
    </button>
  );
}
