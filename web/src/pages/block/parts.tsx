// Small pieces the block and blocks pages share: loading placeholders and resource guards.
import type { Resource } from "solid-js";
import "./parts.css";

/** A resource's value once it has one, without suspending the page while it loads. */
export const ready = <T,>(r: Resource<T>): T | undefined => (r.state === "ready" ? r() : undefined);

/** A resource's error, if its last load failed. */
export const failed = (r: Resource<unknown>): unknown =>
  r.state === "errored" ? (r.error ?? new Error("failed")) : undefined;

/** A grey bar the shape of a line of text that is still loading. */
export function Skel(props: { size?: "s" | "m" | "l" }) {
  return <span class={`block-skel block-skel--${props.size ?? "m"}`} aria-hidden="true" />;
}
