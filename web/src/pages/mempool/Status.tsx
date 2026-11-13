import type { Mined } from "../../lib/replacements";
import "./Status.css";

const WORDS: Record<Mined, string> = { mined: "Mined", waiting: "Not mined", unknown: "Unknown" };

/** Whether a story's newest version is in a block, as an icon and a word (the home page uses it too). */
export function MinedStatus(props: { mined: Mined }) {
  return (
    <span
      class={`rbf-status__word rbf-status__word--${props.mined}`}
      title={
        props.mined === "unknown"
          ? "A reorganisation took its block away; the server has not yet learned where it is."
          : undefined
      }
    >
      <svg class="rbf-status__icon" viewBox="0 0 16 16" aria-hidden="true">
        {props.mined === "mined" ? (
          <path d="M4.5 8.5l2.4 2.4L11.8 5.6" />
        ) : props.mined === "waiting" ? (
          <circle cx="8" cy="8" r="5.5" stroke-dasharray="2.6 2" />
        ) : (
          <path d="M6 6.2a2 2 0 1 1 2.6 1.9c-.4.2-.6.5-.6.9v.5M8 11.3v.2" />
        )}
      </svg>
      {WORDS[props.mined]}
    </span>
  );
}
