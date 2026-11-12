import { type JSX, Show } from "solid-js";
import "./SectionHeader.css";

/** A section heading over a 2 px rule. `children` sit at the right end (counts, a live dot, controls). */
export function SectionHeader(props: { title: string; id?: string; children?: JSX.Element }) {
  return (
    <div class="section__head">
      <h2 id={props.id}>{props.title}</h2>
      <Show when={props.children}>
        <div class="section__meta">{props.children}</div>
      </Show>
    </div>
  );
}
