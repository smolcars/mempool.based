// Small pieces the address page's sections share.
import type { JSX, Resource } from "solid-js";

/**
 * A resource's value once it has one, also while it refreshes; undefined while it first loads or
 * after it failed. Never suspends, so the page frame stays up while sections load.
 */
export function settled<T>(r: Resource<T>): T | undefined {
  return r.state === "ready" || r.state === "refreshing" ? r.latest : undefined;
}

/** A bar where a value will be, as tall as the line it sits in. */
export function Skeleton(props: { width: string }) {
  return <span class="skeleton" style={{ "--w": props.width }} aria-hidden="true" />;
}

/** The unconfirmed state: dashed circle and a word, in the caution color. */
export function UnconfirmedMark(props: { label?: string }) {
  return (
    <span class="address-pending">
      <svg viewBox="0 0 16 16" aria-hidden="true">
        <circle cx="8" cy="8" r="5.5" />
      </svg>
      {props.label ?? "Unconfirmed"}
    </span>
  );
}

/** Important state that is not an error: what is going on, and what it means for the reader. */
export function Notice(props: { title: string; children: JSX.Element; tone?: "caution" }) {
  return (
    <div class="address-notice" classList={{ "address-notice--caution": props.tone === "caution" }}>
      <svg class="address-notice__icon" viewBox="0 0 16 16" aria-hidden="true">
        <circle cx="8" cy="8" r="6.5" />
        <path d="M8 7.5v4M8 4.5v.5" />
      </svg>
      <div class="address-notice__body">
        <p class="address-notice__title">{props.title}</p>
        <div class="address-notice__text">{props.children}</div>
      </div>
    </div>
  );
}
