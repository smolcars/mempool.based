import type { JSX } from "@solidjs/web";
import "./More.css";

/**
 * The second reading layer: how a figure is measured, and the longer notes, behind a disclosure a
 * keyboard reaches. Keep the one statement that changes what a number means outside it.
 */
export function More(props: { label?: string; children: JSX.Element }) {
  return (
    <details class="more">
      <summary class="more__summary">{props.label ?? "How these figures are made"}</summary>
      <div class="more__body">{props.children}</div>
    </details>
  );
}
