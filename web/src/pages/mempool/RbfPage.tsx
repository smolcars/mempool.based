// Recent replacements (RBF), each drawn as its story: the versions in the order they came, how
// long each stood, the rate each paid, and whether the last one was mined (docs/DESIGN.md
// "Replacements"). The data and the live topic are lib/replacements.ts.
import { useSearchParams } from "@solidjs/router";
import { createEffect, createMemo, createSignal, For, Show } from "solid-js";
import { type Column, DataTable } from "../../components/DataTable";
import { FeeRate } from "../../components/FeeRate";
import { Hash } from "../../components/Hash";
import { Link } from "../../components/Link";
import { More } from "../../components/More";
import { SectionHeader } from "../../components/SectionHeader";
import { Empty, ErrorState, Loading } from "../../components/States";
import { TimeAgo } from "../../components/Time";
import { useLiveStatus } from "../../lib/live";
import { feeColor, formatFeeRate, groupDigits } from "../../lib/format";
import { type Network, paths } from "../../lib/network";
import { useNetwork, usePageTitle } from "../../lib/page";
import { createReplacements, formatStood, type Story, type TreeTx } from "../../lib/replacements";
import { MinedStatus } from "./Status";
import "./MempoolPage.css";
import "./RbfPage.css";

/** Versions shown before the middle of a longer story folds away. */
const FOLD_AT = 4;
/** Other replaced transactions named before the rest fold away. */
const OTHERS_SHOWN = 3;
/** Rows held while the first answer is on its way, so the panel keeps its height. */
const BONES = 6;

export default function RbfPage() {
  usePageTitle(() => "Replacements");
  const network = useNetwork();
  return (
    <Show when={network()} keyed>
      {(n) => <Rbf network={n} />}
    </Show>
  );
}

function Rbf(props: { network: Network }) {
  const [search] = useSearchParams();
  const full = () => search["view"] === "full-rbf";
  let list: HTMLElement | undefined;
  const feed = createReplacements(props.network, full, {
    gapMs: 1500,
    around: () => keepPlace(list),
  });
  createEffect(full, (_, before) => {
    if (before !== undefined) feed.refresh();
  });
  const status = useLiveStatus();

  // Stories that came in after the first answer flash once, as the latest transactions do.
  const fresh = new WeakSet<Story>();
  let seen: Set<string> | undefined;
  let seenFull: boolean | undefined;
  const rows = createMemo(() => {
    const s = feed.stories();
    if (!s) return undefined;
    const f = full();
    if (seen && seenFull === f) for (const x of s) if (!seen.has(x.id)) fresh.add(x);
    seen = new Set(s.map((x) => x.id));
    seenFull = f;
    return s;
  });

  return (
    <div class="wrap mempool rbf">
      <nav class="mempool-nav" aria-label="Mempool">
        <Link href={paths.mempool()}>← Mempool history</Link>
      </nav>
      <header class="mempool-head">
        <h1>Replacements</h1>
        <nav class="mempool-intervals" aria-label="Which replacements">
          <ul>
            <li>
              <Link
                class="mempool-intervals__item"
                href={paths.rbf()}
                aria-current={full() ? "false" : "page"}
              >
                All
              </Link>
            </li>
            <li>
              <Link
                class="mempool-intervals__item"
                href={paths.rbf(true)}
                aria-current={full() ? "page" : "false"}
              >
                Full RBF only
              </Link>
            </li>
          </ul>
        </nav>
      </header>
      <p class="rbf-lead">
        <Lead stories={rows()} full={full()} off={feed.off()} />
      </p>
      <section class="rbf-list" aria-labelledby="rbf-title" ref={list}>
        <SectionHeader
          title={full() ? "Replaced without signalling" : "Newest first"}
          id="rbf-title"
        >
          <Show when={!feed.off()}>
            <span class={["rbf-live", { "rbf-live--on": status() === "live" }]}>
              <span class="rbf-live__dot" aria-hidden="true" />
              {status() === "live" ? "Live" : `Checking every 30 s`}
            </span>
          </Show>
        </SectionHeader>
        <Show
          when={!feed.off()}
          fallback={
            <Empty title="Replacements are not recorded here">
              This server does not follow this network's mempool, so it sees no replacements.
            </Empty>
          }
        >
          <Show
            when={rows() !== undefined || feed.error() === undefined}
            fallback={
              <ErrorState
                title="Could not load the replacements"
                error={feed.error()}
                retry={feed.refresh}
              />
            }
          >
            <div class="rbf-slot" aria-busy={rows() === undefined ? "true" : "false"}>
              <Show when={rows() === undefined}>
                <Loading label="Loading the replacements" />
              </Show>
              <DataTable<Story | null>
                caption={
                  full()
                    ? "Replacements of a transaction that did not signal, newest first"
                    : "Replacements, newest first"
                }
                columns={COLUMNS}
                rows={rows() ?? Array.from({ length: BONES }, () => null)}
                rowHref={(s) => (s ? paths.tx(s.id) : undefined)}
                rowClass={(s) => (s && fresh.has(s) ? "rbf-row--new" : undefined)}
                empty={
                  <Empty
                    title={
                      full()
                        ? "No replacement of a transaction that did not signal yet"
                        : "No replacements yet"
                    }
                  >
                    A transaction replaced in the mempool appears here as the server sees it.
                    <Show when={full()}>
                      {" "}
                      <Link href={paths.rbf()}>Show every replacement</Link>
                    </Show>
                  </Empty>
                }
              />
            </div>
          </Show>
        </Show>
      </section>
      <More label="How this list is made">
        <p>
          A replacement is a transaction that spends the same coins as one already waiting and pays
          more, so the node drops the first. Each row is one story: every version, oldest first, how
          long each stood before the next took its place, and the fee rate each paid. The last
          version is the one that stands; a row opens it.
        </p>
        <p>
          A replacement can remove more than one transaction: the children of what it replaced leave
          with it, and it can conflict with several at once. The row follows the longest line of
          versions and names the rest as also replaced.
        </p>
        <p>
          Full RBF means a replaced transaction did not signal that it could be replaced (BIP 125).
          rbitcoin replaces either way, so this is a filter on signalling, not on policy.
        </p>
        <p>
          Mined says the newest version is in a block. When a miner confirms a version the server
          had seen replaced, that version is the newest end of its story: the block replaced the
          rest. Unknown says a reorganisation took its block away and the server has not yet learned
          where it is.
        </p>
        <p>
          The list holds the 25 newest stories from what this server has seen since it first ran. A
          version the server never held (mined before it reached the mempool) has no rate.
        </p>
      </More>
    </div>
  );
}

/** The answer above the list: how many, and how many were mined. One line, its height kept. */
function Lead(props: { stories: Story[] | undefined; full: boolean; off: boolean }) {
  const counts = () => {
    const s = props.stories ?? [];
    return {
      n: s.length,
      mined: s.filter((x) => x.mined === "mined").length,
      newest: s[0]?.time,
    };
  };
  return (
    <Show when={!props.off && props.stories && counts().n > 0} fallback={" "}>
      <span class="num">{groupDigits(counts().n)}</span>{" "}
      {props.full ? "replacements of a transaction that did not signal" : "replacements"}, the
      newest <TimeAgo time={counts().newest ?? 0} />;{" "}
      <Show when={counts().mined > 0} fallback="none of them mined yet.">
        <span class="num">{groupDigits(counts().mined)}</span> of them mined.
      </Show>
    </Show>
  );
}

const COLUMNS: Column<Story | null>[] = [
  {
    header: "What replaced what, oldest first, sat/vB",
    short: "Oldest first, sat/vB",
    cell: (s) => (s ? <StoryCell story={s} /> : <span class="rbf-bone" />),
  },
  {
    header: "Rate rose, sat/vB",
    short: "Rose",
    numeric: true,
    drop: 1,
    cell: (s) => (s ? <Rise rise={s.rise} /> : ""),
  },
  {
    header: "Status",
    cell: (s) => (s ? <StatusCell story={s} /> : <span class="rbf-bone rbf-bone--short" />),
  },
];

function Rise(props: { rise: number | undefined }) {
  const text = () => {
    const r = props.rise;
    if (r === undefined) return "–";
    const printed = formatFeeRate(Math.abs(r));
    // A rate can fall when the replacement pays more in all, over more bytes.
    return `${r < 0 && printed !== "0.00" ? "−" : "+"}${printed}`;
  };
  return (
    <span class="num">
      {text()}
      <span class="visually-hidden"> sat/vB</span>
    </span>
  );
}

function StatusCell(props: { story: Story }) {
  return (
    <div class="rbf-status">
      <MinedStatus mined={props.story.mined} />
      <span class="rbf-status__when">
        <TimeAgo time={props.story.time} />
      </span>
      <Show when={props.story.fullRbf}>
        <span class="rbf-status__full">Full RBF</span>
      </Show>
    </div>
  );
}

/** One version: its mark in its fee colour on the line, its id and its rate. */
function VersionItem(props: { tx: TreeTx; after: number | undefined; newest: boolean }) {
  const rate = () => props.tx.rate;
  return (
    <li class={["rbf-v", { "rbf-v--newest": props.newest }]}>
      <Show when={props.after}>
        {(s) => (
          <span class="rbf-v__after">
            <span class="visually-hidden">replaced after </span>
            {formatStood(s())}
          </span>
        )}
      </Show>
      {/* The version's own target: its link covers its mark, id and rate, a thumb high. */}
      <span class="rbf-v__body">
        <span
          class={["rbf-v__mark", { "rbf-v__mark--none": rate() === null }]}
          style={{ "--c": rate() === null ? "transparent" : feeColor(rate() ?? 0) }}
          aria-hidden="true"
        />
        <span class="rbf-v__id">
          <Hash
            value={props.tx.txid}
            href={paths.tx(props.tx.txid)}
            head={8}
            tail={4}
            copy={false}
          />
        </span>
        <span class="rbf-v__rate">
          <Show when={rate()} keyed fallback={<span class="num">–</span>}>
            {(r) => <FeeRate rate={r} unit={false} />}
          </Show>
          <Show when={props.tx.rbf === false}>
            <span class="rbf-v__nosig">did not signal</span>
          </Show>
        </span>
      </span>
    </li>
  );
}

function StoryCell(props: { story: Story }) {
  const [open, setOpen] = createSignal(false);
  const [all, setAll] = createSignal(false);
  const versions = () => props.story.versions;
  const folded = () => versions().length > FOLD_AT && !open();
  const hidden = () => versions().length - 3;
  const head = () => (folded() ? versions().slice(0, 1) : versions());
  const tail = () => (folded() ? versions().slice(-2) : []);
  const others = () => (all() ? props.story.others : props.story.others.slice(0, OTHERS_SHOWN));
  const count = versions().length;
  return (
    <div class="rbf-story" data-story={props.story.id}>
      <ol class="rbf-story__line" aria-label={`${count} versions, oldest first`}>
        <For each={head()}>
          {(v, i) => (
            <VersionItem
              tx={v.tx}
              after={i() > 0 ? versions()[i() - 1]?.stood : undefined}
              newest={!folded() && i() === versions().length - 1}
            />
          )}
        </For>
        <Show when={folded()}>
          <li class="rbf-story__fold">
            <button
              class="rbf-story__button"
              type="button"
              aria-expanded="false"
              onClick={() => setOpen(true)}
            >
              {hidden()} more versions
            </button>
          </li>
          <For each={tail()}>
            {(v, i) => (
              <VersionItem
                tx={v.tx}
                after={i() > 0 ? versions()[versions().length - 2]?.stood : undefined}
                newest={i() === 1}
              />
            )}
          </For>
        </Show>
      </ol>
      <Show when={props.story.others.length > 0}>
        <p class="rbf-story__others">
          <span class="rbf-story__also">Also replaced:</span>
          <For each={others()}>
            {(t) => <Hash value={t.txid} href={paths.tx(t.txid)} head={8} tail={4} copy={false} />}
          </For>
          <Show when={props.story.others.length > OTHERS_SHOWN && !all()}>
            <button class="rbf-story__button" type="button" onClick={() => setAll(true)}>
              {props.story.others.length - OTHERS_SHOWN} more
            </button>
          </Show>
        </p>
      </Show>
    </div>
  );
}

/**
 * Keeps what the reader is looking at where it is when stories come in above it: the first story
 * in sight stays at the same height on screen. With the list's top in sight, new stories come in
 * at the top, where the reader is looking. Returns what to do once the change is drawn.
 */
function keepPlace(list: HTMLElement | undefined): (() => void) | undefined {
  if (!list) return undefined;
  const top = document.querySelector(".site-header")?.getBoundingClientRect().bottom ?? 0;
  const items = [...list.querySelectorAll<HTMLElement>("[data-story]")];
  const first = items[0];
  if (!first || first.getBoundingClientRect().top >= top) return undefined;
  const anchor = items.find((el) => el.getBoundingClientRect().top >= top);
  if (!anchor) return undefined;
  // The rows from it down with where each stood: if it moved (a story that grew goes to the
  // top), the next one still here holds the place.
  const held = items
    .slice(items.indexOf(anchor))
    .map((el) => [el.dataset["story"] ?? "", el.getBoundingClientRect().top] as const);
  return () => {
    for (const [id, was] of held) {
      const el = list.querySelector<HTMLElement>(`[data-story="${CSS.escape(id)}"]`);
      if (!el) continue;
      window.scrollBy(0, el.getBoundingClientRect().top - was);
      return;
    }
  };
}
