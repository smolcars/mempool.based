import { type Accessor, Show } from "solid-js";
import "./Footer.css";

export function Footer(props: { version: Accessor<string | undefined> }) {
  return (
    <footer class="site-footer">
      <div class="site-footer__inner">
        <p>rbitcoin.space, a block explorer for rbitcoin.</p>
        <p>
          <a
            class="tap"
            href="https://github.com/smolcars/rbitcoin-space"
            rel="noopener noreferrer"
          >
            Source code
          </a>
          {/* Last on the page, so its late arrival moves nothing. */}
          <Show when={props.version()}>
            {(v) => <span class="muted num">, version {v()}</span>}
          </Show>
        </p>
      </div>
    </footer>
  );
}
