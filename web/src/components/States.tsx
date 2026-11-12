import { type JSX, Show } from "solid-js";
import { ApiError } from "../lib/api";
import "./Button.css";
import "./States.css";

/**
 * Something is on its way. Strata settle into a block while it waits; the whole thing fades in
 * only after a moment, so fast loads never flash it.
 */
export function Loading(props: { label?: string }) {
  return (
    <div class="state state--loading" role="status">
      <span class="loading-mark" aria-hidden="true">
        <span class="loading-mark__layer" />
        <span class="loading-mark__layer" />
        <span class="loading-mark__layer" />
      </span>
      <span class="state__text">{props.label ?? "Loading"}</span>
    </div>
  );
}

/** Nothing to show. Say what would be here and, in `children`, what the reader can do. */
export function Empty(props: { title: string; children?: JSX.Element }) {
  return (
    <div class="state state--empty">
      <p class="state__title">{props.title}</p>
      <Show when={props.children}>
        <div class="state__text">{props.children}</div>
      </Show>
    </div>
  );
}

/**
 * What went wrong and how to recover. `title` names what failed: "Could not load this block".
 * `message` replaces the error's own.
 */
export function ErrorState(props: {
  title: string;
  error?: unknown;
  message?: string;
  retry?: () => void;
}) {
  const message = () =>
    props.message ??
    (props.error instanceof ApiError
      ? props.error.message
      : "Something went wrong while showing this.");
  return (
    <div class="state state--error" role="alert">
      <p class="state__title">
        <svg class="state__icon" viewBox="0 0 16 16" aria-hidden="true">
          <circle cx="8" cy="8" r="6.5" />
          <path d="M8 4.5v4M8 11v.5" />
        </svg>
        {props.title}
      </p>
      <p class="state__text">{message()}</p>
      <Show when={props.retry}>
        {(retry) => (
          <button class="btn" type="button" onClick={() => retry()()}>
            Try again
          </button>
        )}
      </Show>
    </div>
  );
}
