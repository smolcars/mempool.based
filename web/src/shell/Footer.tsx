import { type Accessor, Show } from "solid-js";
import "./Footer.css";

export function Footer(props: { version: Accessor<string | undefined> }) {
  return (
    <footer class="site-footer">
      <div class="site-footer__inner">
        <p>mempool.based, a block explorer for rbitcoin.</p>
        <p>
          <a href="https://github.com/smolcars/mempool.based" rel="noopener noreferrer">
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
