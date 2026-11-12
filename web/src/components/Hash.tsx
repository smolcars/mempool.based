import { For, Show } from "solid-js";
import { hashGroups, truncateMiddle } from "../lib/format";
import { CopyButton } from "./CopyButton";
import { Link } from "./Link";
import "./Hash.css";

export interface HashProps {
  value: string;
  /** Network-free page path (from `paths`) to link to. */
  href?: string;
  /** Characters kept at each end: 8 and 8 in lists, 7 and 5 for addresses in the flow diagram. */
  head?: number;
  tail?: number;
  /** Copy button; on by default. */
  copy?: boolean;
  /** What the copy button copies, for its label: "Copy transaction ID". */
  copyLabel?: string;
  /** The whole value, wrapping where it must, instead of a shortened one. */
  whole?: boolean;
}

/** A hash or address shortened in the middle, in the monospace cut. Full value in the title and the copy. */
export function Hash(props: HashProps) {
  const short = () =>
    props.whole ? props.value : truncateMiddle(props.value, props.head, props.tail);
  return (
    <span class={["hash", { "hash--whole": props.whole === true }]}>
      <Show
        when={props.href}
        fallback={
          <span class="hash__text" title={props.value}>
            {short()}
          </span>
        }
      >
        {(href) => (
          <Link class="hash__text" href={href()} title={props.value}>
            {short()}
          </Link>
        )}
      </Show>
      <Show when={props.copy !== false}>
        <CopyButton value={props.value} {...(props.copyLabel ? { label: props.copyLabel } : {})} />
      </Show>
    </span>
  );
}

/** A hash shown whole as a page subject, in groups of eight, with a copy button. */
export function HashFull(props: { value: string; copyLabel?: string }) {
  return (
    <span class="hash hash--full">
      <span class="hash__text">
        <For each={hashGroups(props.value)}>{(g) => <span class="hash__group">{g}</span>}</For>
      </span>
      <CopyButton value={props.value} {...(props.copyLabel ? { label: props.copyLabel } : {})} />
    </span>
  );
}
