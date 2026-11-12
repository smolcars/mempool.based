import { createSignal, Show } from "solid-js";
import { CopyButton } from "../../components/CopyButton";
import { formatSize } from "../../lib/format";
import { PREVIEW_CHARS, SHOW_ALL_MAX } from "./tx";

/**
 * Hex, assembly or decoded text from the chain, as text only. Shows the first PREVIEW_CHARS
 * characters; "Show all" lays out the rest up to SHOW_ALL_MAX, and anything longer is copy only.
 */
export function Code(props: {
  label: string;
  value: string;
  /** Hex is copied whole; set false for derived text such as assembly. */
  copy?: boolean;
  /** The value was itself cut short upstream (assembly of a huge script). */
  cut?: boolean;
  /** Byte count to print for hex, which is half its length. */
  bytes?: number;
}) {
  const [all, setAll] = createSignal(false);
  const long = () => props.value.length > PREVIEW_CHARS;
  const shown = () => (all() || !long() ? props.value : `${props.value.slice(0, PREVIEW_CHARS)}…`);
  const size = () => (props.bytes === undefined ? undefined : formatSize(props.bytes, "B"));
  return (
    <div class="tx-code">
      <div class="tx-code__head">
        <span class="tx-code__label">{props.label}</span>
        <Show when={size()}>{(s) => <span class="tx-code__size num">{s()}</span>}</Show>
        <Show when={props.copy !== false && props.value !== ""}>
          <CopyButton value={props.value} label={`Copy ${props.label.toLowerCase()}`} />
        </Show>
      </div>
      <Show when={props.value !== ""} fallback={<p class="tx-code__empty">Empty</p>}>
        <pre class="tx-code__text" tabindex="0">
          {shown()}
        </pre>
      </Show>
      <Show when={long() || props.cut}>
        <Show
          when={props.value.length <= SHOW_ALL_MAX && !props.cut}
          fallback={
            <p class="tx-code__note">
              Too long to show in full here.{props.copy !== false ? " Copy it instead." : ""}
            </p>
          }
        >
          <button class="btn tx-code__more" type="button" onClick={() => setAll(!all())}>
            {all() ? "Show less" : "Show all"}
          </button>
        </Show>
      </Show>
    </div>
  );
}
