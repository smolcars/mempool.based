import { useBeforeLeave, useNavigate } from "@solidjs/router";
import { createEffect, createMemo, createSignal, onCleanup, onSettled, Show } from "solid-js";
import { ApiError } from "../lib/api";
import { groupDigits, truncateMiddle } from "../lib/format";
import { NETWORK_NAMES, onNetwork, paths } from "../lib/network";
import { useNetwork } from "../lib/page";
import type { Logic, Outcome, Query } from "../search/search";
import { Link } from "./Link";
import "./Button.css";
import "./SearchField.css";

/** classify() refuses anything longer, and the field holds no more (search/query.ts MAX_QUERY). */
const MAX_LENGTH = 512;

export interface SearchFieldProps {
  id?: string;
  ref?: (el: HTMLInputElement) => void;
}

type Miss = Exclude<Outcome, { kind: "go" }>;

const queried = (miss: Miss) => (miss.kind === "missing" ? miss.query : null);

/** What a finished search that leads nowhere says, in one sentence for screen readers. */
function summary(miss: Miss, network: string): string {
  if (miss.kind === "failed") return "Search could not finish.";
  return miss.query?.kind === "hash"
    ? `Nothing matches: no block or transaction on ${network} has this ID.`
    : "Nothing matches: this is not a transaction ID, block hash, height or address.";
}

/**
 * The header's search. Reads the query as it is typed, and on Enter goes to its page or explains
 * why there is none. The logic (search/search.ts) loads when the field is first used.
 */
/** A phone's resting header gives the search little room: its hint is one word there. */
const PHONE = "(max-width: 640px)";

export function SearchField(props: SearchFieldProps) {
  const id = () => props.id ?? "search";
  const phone = typeof matchMedia === "function" ? matchMedia(PHONE) : undefined;
  const [narrow, setNarrow] = createSignal(phone?.matches === true);
  onSettled(() => {
    const change = (e: MediaQueryListEvent) => setNarrow(e.matches);
    phone?.addEventListener("change", change);
    return () => phone?.removeEventListener("change", change);
  });
  const network = useNetwork();
  const navigate = useNavigate();
  const [text, setText] = createSignal("");
  const [logic, setLogic] = createSignal<Logic>();
  const [busy, setBusy] = createSignal(false);
  const [miss, setMiss] = createSignal<{ query: string; miss: Miss }>();
  const [said, setSaid] = createSignal("");
  // A well-formed ID that nothing on this network has: the field says so, not what it looks like.
  const [nothingFor, setNothingFor] = createSignal<string>();
  let root: HTMLElement | undefined;
  let input: HTMLInputElement | undefined;

  let loading: Promise<Logic> | undefined;
  let searcher: ReturnType<Logic["createSearch"]> | undefined;
  const load = () => {
    loading ??= (async () => {
      try {
        const m = await import("../search/search");
        searcher ??= m.createSearch();
        setLogic(() => m);
        return m;
      } catch (error) {
        loading = undefined;
        throw error;
      }
    })();
    return loading;
  };
  const warm = () => void load().catch(() => undefined);

  const reading = createMemo(() => {
    const m = logic();
    const q = text().trim();
    if (q !== "" && q === nothingFor()) return { label: "Nothing has this ID", ready: false };
    return m && q !== "" ? m.describe(q, network()) : undefined;
  });

  // The reading is announced once typing pauses, and only when it changed: not every keystroke.
  let pause: ReturnType<typeof setTimeout> | undefined;
  createEffect(reading, (r) => {
    clearTimeout(pause);
    if (r) pause = setTimeout(() => setSaid(r.label), 900);
  });
  onCleanup(() => clearTimeout(pause));

  // Every new search, edit, or navigation makes the search in flight stale.
  let generation = 0;
  const stop = () => {
    generation += 1;
    searcher?.cancel();
    setBusy(false);
    setMiss(undefined);
  };
  // Navigation start, not end: a page still loading must not be overridden by a search answering.
  // The query goes too: on the next page it would read as that page's, "Not recognised" and all.
  useBeforeLeave(() => {
    stop();
    setText("");
    setNothingFor(undefined);
  });

  const search = async (query: string) => {
    stop();
    const mine = generation;
    const net = network();
    setBusy(true);
    let outcome: Outcome | undefined;
    try {
      await load();
      if (mine !== generation) return;
      outcome = await searcher?.run(query, net);
    } catch (error) {
      outcome = { kind: "failed", error };
    }
    if (outcome === undefined || mine !== generation) return;
    setBusy(false);
    if (outcome.kind === "go") {
      setText("");
      input?.blur();
      navigate(onNetwork(net, outcome.path));
    } else {
      setMiss({ query, miss: outcome });
      if (outcome.kind === "missing" && outcome.query?.kind === "hash") setNothingFor(query);
      setSaid(summary(outcome, NETWORK_NAMES[net]));
    }
  };

  const submit = (e: SubmitEvent) => {
    e.preventDefault();
    const query = text().trim();
    if (query !== "") void search(query);
  };

  // A miss closes when the reader moves on: a press outside, or focus leaving the search.
  const outside = (target: EventTarget | null) =>
    target instanceof Node && root !== undefined && !root.contains(target);
  onSettled(() => {
    const onPress = (e: PointerEvent) => {
      if (outside(e.target)) setMiss(undefined);
    };
    document.addEventListener("pointerdown", onPress);
    return () => document.removeEventListener("pointerdown", onPress);
  });

  return (
    <search
      class="search"
      ref={(el) => (root = el)}
      onFocusOut={(e) => {
        if (outside(e.relatedTarget)) setMiss(undefined);
      }}
    >
      <form class="search__field" onSubmit={submit}>
        <label class="visually-hidden" for={id()}>
          Search
        </label>
        <svg class="search__icon" viewBox="0 0 16 16" aria-hidden="true">
          <circle cx="7" cy="7" r="4.5" />
          <path d="M10.5 10.5 14 14" />
        </svg>
        <input
          ref={(el) => {
            input = el;
            // Set here: Solid writes a literal "/" into its template unquoted, which some parsers drop.
            el.setAttribute("aria-keyshortcuts", "/");
            props.ref?.(el);
          }}
          id={id()}
          name="q"
          type="search"
          placeholder={narrow() ? "Search" : "Transaction, address or block"}
          autocomplete="off"
          spellcheck="false"
          enterkeyhint="search"
          maxlength={MAX_LENGTH}
          aria-describedby={`${id()}-reading`}
          value={text()}
          onFocus={warm}
          onPointerEnter={warm}
          onInput={(e) => {
            setText(e.currentTarget.value);
            setNothingFor(undefined);
            stop();
          }}
          onKeyDown={(e) => {
            if (e.key === "Escape" && miss()) {
              e.preventDefault();
              setMiss(undefined);
            }
          }}
        />
        <span id={`${id()}-reading`} class="search__reading">
          <Show
            when={!busy()}
            fallback={
              <span class="search__busy">
                <span class="search__pulse" aria-hidden="true" />
                Looking up
              </span>
            }
          >
            <Show when={reading()}>
              {(r) => (
                <span class={["search__label", { "search__label--ready": r().ready }]}>
                  {r().label}
                  <Show when={r().ready}>
                    <kbd class="search__key" title="Press Enter to search">
                      <svg viewBox="0 0 12 12" aria-hidden="true">
                        <path d="M10 2v4.5H2.5M5 4 2.5 6.5 5 9" />
                      </svg>
                      <span class="visually-hidden">Enter</span>
                    </kbd>
                  </Show>
                </span>
              )}
            </Show>
          </Show>
        </span>
        <kbd class="search__slash" aria-hidden="true" title="Press / to search">
          /
        </kbd>
      </form>
      <span class="visually-hidden" role="status">
        {said()}
      </span>
      <Show when={miss()}>
        {(m) => (
          <div class="search__panel" tabindex="-1">
            <Show
              when={m().miss.kind === "missing"}
              fallback={
                <Failed
                  miss={m().miss}
                  retry={() => {
                    void search(m().query);
                  }}
                />
              }
            >
              <Nothing
                query={m().query}
                found={queried(m().miss)}
                network={NETWORK_NAMES[network()]}
              />
            </Show>
          </div>
        )}
      </Show>
    </search>
  );
}

function Nothing(props: { query: string; found: Query | null; network: string }) {
  return (
    <>
      <p class="search__title">Nothing matches</p>
      <Show
        when={props.found?.kind === "hash" && props.found.hash}
        fallback={
          <p>
            <q class="search__query">{truncateMiddle(props.query, 28, 12)}</q> is not a transaction
            ID, block hash, block height or address.
          </p>
        }
      >
        {(hash) => (
          <p>
            No block or transaction on {props.network} has the ID{" "}
            <span class="search__query search__query--hash" title={hash()}>
              {truncateMiddle(hash(), 12, 12)}
            </span>
            .
          </p>
        )}
      </Show>
      <dl class="search__finds">
        <dt>Transaction</dt>
        <dd>its ID, 64 characters</dd>
        <dt>Block</dt>
        <dd>
          its hash, or its height such as{" "}
          <Link href={paths.block(840_000)}>{groupDigits(840_000)}</Link>
        </dd>
        <dt>Address</dt>
        <dd>the whole address</dd>
      </dl>
      <p class="search__aside">
        Search finds exact matches on {props.network} only, never part of an ID or an address. For
        another network, switch to it first, or browse the{" "}
        <Link href={paths.blocks()}>latest blocks</Link>.
      </p>
    </>
  );
}

function Failed(props: { miss: Miss; retry: () => void }) {
  const message = () => {
    const e = props.miss.kind === "failed" ? props.miss.error : undefined;
    return e instanceof ApiError
      ? e.message
      : "The search could not be loaded. Check your connection.";
  };
  return (
    <>
      <p class="search__title">Search could not finish</p>
      <p>{message()}</p>
      <button type="button" class="btn" onClick={() => props.retry()}>
        Try again
      </button>
    </>
  );
}
