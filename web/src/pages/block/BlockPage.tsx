import { useParams } from "@solidjs/router";
import {
  createEffect,
  createMemo,
  createSignal,
  For,
  type JSX,
  Match,
  on,
  onCleanup,
  Show,
  Switch,
} from "solid-js";
import { Amount } from "../../components/Amount";
import { DataTable } from "../../components/DataTable";
import { FeeRate } from "../../components/FeeRate";
import { Hash, HashFull } from "../../components/Hash";
import { Link } from "../../components/Link";
import { SectionHeader } from "../../components/SectionHeader";
import { ErrorState } from "../../components/States";
import { TimeAgo, TimeUtc } from "../../components/Time";
import { createApi, fetchApi, isNotFound } from "../../lib/api";
import type { Block, BlockStatus, Transaction } from "../../lib/esplora";
import {
  feeRate,
  formatSats,
  formatSize,
  groupDigits,
  MISSING,
  truncateMiddle,
  vsizeFromWeight,
} from "../../lib/format";
import { NETWORK_NAMES, paths } from "../../lib/network";
import { useNetwork, usePageTitle } from "../../lib/page";
import {
  blockReward,
  coinbaseTag,
  formatDifficulty,
  fullness,
  HALVING_INTERVAL,
  hex32,
  parseBlockId,
  shownParam,
} from "./chain";
import "./BlockPage.css";
import { failed, ready, Skel } from "./parts";

/** The API serves a block's transactions 25 at a time. */
const PAGE = 25;

const percent = (share: number) => {
  const p = share * 100;
  return `${p >= 99.995 ? "100" : p.toFixed(p < 1 ? 2 : 1)}%`;
};

/** `/block/:id`, where `id` is a block hash or a height. */
export default function BlockPage() {
  const params = useParams<{ id: string }>();
  const id = createMemo(() => parseBlockId(params.id));
  const [byHeight, byHeightCtl] = createApi<string>(
    () => {
      const i = id();
      return i?.kind === "height" && `/block-height/${i.height}`;
    },
    { as: "text" },
  );
  const hash = () => {
    const i = id();
    if (i?.kind === "hash") return i.hash;
    const h = i && ready(byHeight);
    return h && /^[0-9a-f]{64}$/.test(h) ? h : undefined;
  };
  const height = () => {
    const i = id();
    return i?.kind === "height" ? i.height : undefined;
  };
  const heightError = () => (id()?.kind === "height" ? failed(byHeight) : undefined);

  return (
    <Switch fallback={<BlockView hash={hash()} height={height()} />}>
      <Match when={!id()}>
        <BadId value={params.id} />
      </Match>
      <Match when={heightError()}>
        {(e) => (
          <Show
            when={isNotFound(e())}
            fallback={<PageError error={e()} retry={() => void byHeightCtl.refetch()} />}
          >
            <NoBlockAtHeight height={height() ?? 0} />
          </Show>
        )}
      </Match>
    </Switch>
  );
}

function Message(props: { title: string; children: JSX.Element }) {
  usePageTitle(() => props.title);
  return (
    <div class="wrap block-page block-message">
      <h1>{props.title}</h1>
      {props.children}
    </div>
  );
}

function BadId(props: { value: string }) {
  return (
    <Message title="Not a block">
      <p class="block-message__text">
        “{truncateMiddle(shownParam(props.value), 40, 20)}” is not a block hash or a height. A block
        hash is 64 hexadecimal characters, and a height is a whole number such as{" "}
        <Link href={paths.block(840_000)}>840{" "}000</Link>.
      </p>
    </Message>
  );
}

function NoBlockAtHeight(props: { height: number }) {
  const [tip] = createApi<string>(() => "/blocks/tip/height", { as: "text" });
  const tipHeight = () => {
    const t = Number(ready(tip));
    return Number.isSafeInteger(t) ? t : undefined;
  };
  return (
    <Message title={`No block ${groupDigits(props.height)} yet`}>
      <p class="block-message__text">
        Block {groupDigits(props.height)} has not been mined.{" "}
        <Show when={tipHeight()}>
          {(t) => (
            <>
              The newest block is <Link href={paths.block(t())}>{groupDigits(t())}</Link>.
            </>
          )}
        </Show>
      </p>
    </Message>
  );
}

function NotFound(props: { hash: string }) {
  const network = useNetwork();
  return (
    <Message title="Block not found">
      <div class="block-head__hash">
        <HashFull value={props.hash} copyLabel="Copy block hash" />
      </div>
      <p class="block-message__text">
        No block with this hash is known on {NETWORK_NAMES[network()]}. It may belong to another
        network: check the network switch above.
      </p>
    </Message>
  );
}

function PageError(props: { error: unknown; retry: () => void }) {
  usePageTitle(() => "Block");
  return (
    <div class="wrap block-page">
      <h1>Block</h1>
      <ErrorState title="Could not load this block" error={props.error} retry={props.retry} />
    </div>
  );
}

/** One label and value in the facts list. `sub` is a muted second line; space for it is always kept. */
function Fact(props: {
  label: string;
  value: JSX.Element | undefined;
  sub?: JSX.Element;
  /** Spans both columns, or only on phones where the value would not fit in one. */
  wide?: true | "phone";
  /** Keeps two lines for the value, which can wrap (the coinbase tag). */
  tall?: boolean;
}) {
  return (
    <div
      class="block-fact"
      classList={{
        "block-fact--wide": props.wide === true,
        "block-fact--wide-phone": props.wide === "phone",
        "block-fact--tall": props.tall === true,
      }}
    >
      <dt>{props.label}</dt>
      <dd>
        <span class="block-fact__value">{props.value ?? <Skel />}</span>
        <span class="block-fact__sub">{props.sub}</span>
      </dd>
    </div>
  );
}

/** Facts the public API cannot give without reading every transaction; a server route will fill them. */
const pending = () => <span class="block-fact__pending">Not available yet</span>;

function BlockView(props: { hash?: string | undefined; height?: number | undefined }) {
  const network = useNetwork();
  const at = (suffix: string) => () => props.hash && `/block/${props.hash}${suffix}`;
  const [block, blockCtl] = createApi<Block>(at(""));
  const [status, statusCtl] = createApi<BlockStatus>(at("/status"));
  const [first, firstCtl] = createApi<Transaction[]>(at("/txs/0"));
  const [tip] = createApi<string>(() => "/blocks/tip/height", { as: "text" });

  const b = () => ready(block);
  const s = () => ready(status);
  const height = () => b()?.height ?? props.height;
  const stale = () => s()?.in_best_chain === false;
  usePageTitle(() => {
    const h = height();
    return h === undefined
      ? `Block ${truncateMiddle(props.hash ?? "")}`.trim()
      : `Block ${groupDigits(h)}`;
  });

  // Further pages of transactions, appended below the first.
  const [more, setMore] = createSignal<Transaction[]>([]);
  const [moreLoading, setMoreLoading] = createSignal(false);
  const [moreError, setMoreError] = createSignal<unknown>();
  let controller: AbortController | undefined;
  onCleanup(() => controller?.abort());
  createEffect(
    on(
      () => props.hash,
      () => {
        controller?.abort();
        setMore([]);
        setMoreLoading(false);
        setMoreError(undefined);
      },
      { defer: true },
    ),
  );
  const txs = () => {
    const f = ready(first);
    return f ? [...f, ...more()] : undefined;
  };
  const txCount = () => b()?.tx_count;
  const hasMore = () => {
    const shown = txs()?.length ?? 0;
    const total = txCount() ?? 0;
    return shown > 0 && shown < total;
  };
  const loadMore = async () => {
    const hash = props.hash;
    const shown = txs()?.length;
    if (!hash || shown === undefined || moreLoading()) return;
    const c = new AbortController();
    controller = c;
    setMoreLoading(true);
    setMoreError(undefined);
    try {
      const page = await fetchApi<Transaction[]>(network(), `/block/${hash}/txs/${shown}`, {
        signal: c.signal,
      });
      if (!c.signal.aborted) setMore((m) => [...m, ...page]);
    } catch (e) {
      if (!c.signal.aborted) setMoreError(e);
    } finally {
      if (!c.signal.aborted) setMoreLoading(false);
    }
  };

  const coinbase = () => ready(first)?.[0];
  const reward = () => {
    const blk = b();
    if (!blk || ready(first) === undefined) return undefined;
    return blockReward(coinbase(), blk.height, HALVING_INTERVAL[network()]);
  };
  const tag = () => {
    const blk = b();
    const vin = coinbase()?.vin?.[0];
    return blk && vin?.is_coinbase ? coinbaseTag(vin.scriptsig, blk.height) : undefined;
  };
  const confirmations = () => {
    const blk = b();
    const st = s();
    const t = Number(ready(tip));
    if (!blk || !st || !Number.isSafeInteger(t)) return undefined;
    return st.in_best_chain ? Math.max(1, t - blk.height + 1) : 0;
  };
  const pageError = () => failed(block) ?? failed(status);
  const retry = () => {
    if (failed(block) !== undefined) void blockCtl.refetch();
    if (failed(status) !== undefined) void statusCtl.refetch();
    if (failed(first) !== undefined) void firstCtl.refetch();
  };

  const rows = (): readonly (Transaction | null)[] => {
    const list = txs();
    if (list) return list;
    return Array.from({ length: Math.min(PAGE, txCount() ?? PAGE) }, () => null);
  };

  return (
    <Show
      when={pageError() === undefined}
      fallback={
        <Show
          when={isNotFound(pageError()) && props.hash}
          fallback={<PageError error={pageError()} retry={retry} />}
        >
          {(h) => <NotFound hash={h()} />}
        </Show>
      }
    >
      <div class="wrap block-page">
        <nav class="block-pager" aria-label="Neighbouring blocks">
          <span class="block-pager__prev">
            <Show when={b()} fallback={<Skel size="s" />}>
              {(blk) => (
                <Show
                  when={blk().previousblockhash}
                  fallback={<span class="muted">Genesis block</span>}
                >
                  {(prev) => (
                    <Link href={paths.block(prev())} rel="prev">
                      ← Previous: <span class="num">{groupDigits(blk().height - 1)}</span>
                    </Link>
                  )}
                </Show>
              )}
            </Show>
          </span>
          <span class="block-pager__next">
            <Show when={b() && s()} fallback={<Skel size="s" />}>
              <Switch>
                <Match when={stale()}>
                  <span class="muted">Not on the best chain</span>
                </Match>
                <Match when={s()?.next_best}>
                  {(next) => (
                    <Link href={paths.block(next())} rel="next">
                      Next: <span class="num">{groupDigits((height() ?? 0) + 1)}</span>
                      {"\u202F→"}
                    </Link>
                  )}
                </Match>
                <Match when={true}>
                  <span class="muted">The newest block</span>
                </Match>
              </Switch>
            </Show>
          </span>
        </nav>

        <header class="block-head">
          <div class="block-head__title">
            <h1>
              Block{" "}
              <Show when={height() !== undefined} fallback={<Skel size="s" />}>
                <span class="num">{groupDigits(height() ?? 0)}</span>
              </Show>
            </h1>
            <Show when={stale()}>
              <span class="block-status">
                <svg class="block-status__icon" viewBox="0 0 16 16" aria-hidden="true">
                  <path d="M4.5 4.5l7 7M11.5 4.5l-7 7" />
                </svg>
                Stale
              </span>
            </Show>
          </div>
          <div class="block-head__hash">
            <Show
              when={props.hash}
              fallback={
                // The same hash shape with its text hidden, so it wraps exactly as the real one will.
                <span class="block-head__ghost" aria-hidden="true">
                  <HashFull value={"0".repeat(64)} />
                </span>
              }
            >
              {(h) => <HashFull value={h()} copyLabel="Copy block hash" />}
            </Show>
          </div>
        </header>

        <Show when={stale()}>
          <p class="block-notice">
            This block is not on the best chain. Another block took its place at height{" "}
            <Link href={paths.block(height() ?? 0)}>{groupDigits(height() ?? 0)}</Link>, so the
            transactions here are confirmed only if that chain included them too.
          </p>
        </Show>

        <div class="block-layout">
          <CoreSample weight={b()?.weight} />
          <div class="block-side">
            <dl class="block-facts">
              <Fact
                label="Mined"
                wide="phone"
                value={b() ? <TimeUtc time={b()?.timestamp ?? 0} /> : undefined}
                sub={b() && <TimeAgo time={b()?.timestamp ?? 0} />}
              />
              <Fact label="Miner" wide="phone" value={b() ? pending() : undefined} />
              <Fact
                label="Confirmations"
                value={
                  confirmations() === undefined ? undefined : (
                    <span class="num">{groupDigits(confirmations() ?? 0)}</span>
                  )
                }
                sub={stale() ? "Not on the best chain" : undefined}
              />
              <Fact
                label="Transactions"
                value={b() ? <span class="num">{groupDigits(b()?.tx_count ?? 0)}</span> : undefined}
              />
              <Fact
                label="Size"
                value={b() ? <span class="num">{formatSize(b()?.size ?? 0, "B")}</span> : undefined}
                sub={b() && `${formatSize(vsizeFromWeight(b()?.weight ?? 0))} virtual`}
              />
              <Fact
                label="Weight"
                value={
                  b() ? <span class="num">{formatSize(b()?.weight ?? 0, "WU")}</span> : undefined
                }
                sub={b() && `${percent(fullness(b()?.weight ?? 0))} of the limit`}
              />
              <Fact label="Median fee rate" value={b() ? pending() : undefined} />
              <Fact label="Fee range" value={b() ? pending() : undefined} />
              <Fact
                label="Total fees"
                wide="phone"
                value={
                  reward() === undefined ? undefined : (
                    <Show when={typeof reward()?.fees === "number"} fallback={MISSING}>
                      <Amount sats={reward()?.fees ?? 0} />
                    </Show>
                  )
                }
                sub={
                  reward() === null
                    ? "Not available for this block"
                    : reward()?.fees === null
                      ? "The miner claimed less than the subsidy"
                      : reward() && `${formatSats(reward()?.fees ?? 0)} sats`
                }
              />
              <Fact
                label="Reward"
                wide="phone"
                value={
                  reward() === undefined ? undefined : reward() ? (
                    <Amount sats={reward()?.reward ?? 0} />
                  ) : (
                    MISSING
                  )
                }
                sub={
                  reward() && (
                    <>
                      <Amount sats={reward()?.subsidy ?? 0} /> subsidy
                    </>
                  )
                }
              />
              <Fact
                label="Coinbase tag"
                wide
                tall
                value={
                  ready(first) === undefined ? undefined : (
                    <Show when={tag()} fallback={MISSING}>
                      {(t) => (
                        <Show
                          when={t().readable}
                          fallback={
                            <span class="block-tag block-tag--hex">{t().hex || MISSING}</span>
                          }
                        >
                          <span class="block-tag">
                            <For each={t().parts}>
                              {(p) =>
                                "text" in p ? (
                                  p.text
                                ) : (
                                  <span
                                    class="block-tag__hidden"
                                    title={`${p.hidden} ${p.hidden === 1 ? "character" : "characters"} that are not text`}
                                  >
                                    {"·".repeat(p.hidden)}
                                  </span>
                                )
                              }
                            </For>
                          </span>
                        </Show>
                      )}
                    </Show>
                  )
                }
                sub={
                  ready(first) !== undefined &&
                  (tag()?.readable === false
                    ? "No readable text; shown as hex"
                    : tag() && "Written by the miner. Dots stand for bytes that are not text")
                }
              />
            </dl>

            <h2 class="block-side__title">Header</h2>
            <dl class="block-facts block-facts--header">
              <HeaderFact label="Difficulty">
                {(blk) => {
                  const d = formatDifficulty(blk.difficulty);
                  return (
                    <span class="num" title={d.full}>
                      {d.short}
                    </span>
                  );
                }}
              </HeaderFact>
              <HeaderFact label="Version">
                {(blk) => <span class="num">{hex32(blk.version)}</span>}
              </HeaderFact>
              <HeaderFact label="Bits">
                {(blk) => <span class="num">{hex32(blk.bits)}</span>}
              </HeaderFact>
              <HeaderFact label="Nonce">
                {(blk) => <span class="num">{groupDigits(blk.nonce)}</span>}
              </HeaderFact>
              <HeaderFact label="Median time">
                {(blk) => <TimeUtc time={blk.mediantime} />}
              </HeaderFact>
              <HeaderFact label="Merkle root">
                {(blk) => <Hash value={blk.merkle_root} copyLabel="Copy merkle root" />}
              </HeaderFact>
            </dl>
          </div>
        </div>

        <section class="block-txs" aria-labelledby="block-txs-title">
          <SectionHeader title="Transactions" id="block-txs-title">
            <span class="num" aria-live="polite">
              <Show when={txs() && txCount() !== undefined} fallback={<Skel size="s" />}>
                {groupDigits(txs()?.length ?? 0)} of {groupDigits(txCount() ?? 0)}, in block order
              </Show>
            </span>
          </SectionHeader>
          <Show
            when={failed(first) === undefined}
            fallback={
              <ErrorState
                title="Could not load the transactions"
                error={failed(first)}
                retry={() => void firstCtl.refetch()}
              />
            }
          >
            <DataTable caption="Transactions in this block" columns={columns} rows={rows()} />
            <div class="block-txs__more">
              <Show when={moreError()}>
                {(e) => (
                  <ErrorState
                    title="Could not load more transactions"
                    error={e()}
                    retry={() => void loadMore()}
                  />
                )}
              </Show>
              <Show when={hasMore() && moreError() === undefined}>
                <button
                  class="btn"
                  type="button"
                  aria-disabled={moreLoading()}
                  onClick={() => void loadMore()}
                >
                  {moreLoading()
                    ? "Loading…"
                    : `Show ${Math.min(PAGE, (txCount() ?? 0) - (txs()?.length ?? 0))} more`}
                </button>
              </Show>
            </div>
          </Show>
        </section>
      </div>
    </Show>
  );

  function HeaderFact(p: { label: string; children: (blk: Block) => JSX.Element }) {
    return (
      <div class="block-fact">
        <dt>{p.label}</dt>
        <dd>
          {/* Keyed: a cached block can replace another without a loading state in between. */}
          <Show when={b()} keyed fallback={<Skel size="s" />}>
            {(blk) => p.children(blk)}
          </Show>
        </dd>
      </div>
    );
  }
}

const vsize = (tx: Transaction) =>
  tx.weight === undefined ? undefined : vsizeFromWeight(tx.weight);
const isCoinbase = (tx: Transaction) => tx.vin?.[0]?.is_coinbase === true;

const columns = [
  {
    header: "Transaction",
    cell: (tx: Transaction | null) =>
      tx ? (
        <Hash value={tx.txid} href={paths.tx(tx.txid)} copyLabel="Copy transaction ID" />
      ) : (
        <Skel />
      ),
  },
  {
    header: "Fee rate, sat/vB",
    cell: (tx: Transaction | null) => {
      if (!tx) return <Skel size="s" />;
      if (isCoinbase(tx)) return <span class="muted">Coinbase</span>;
      const v = vsize(tx);
      const rate = tx.fee === undefined || v === undefined ? null : feeRate(tx.fee, v);
      return rate === null ? MISSING : <FeeRate rate={rate} unit={false} />;
    },
  },
  {
    header: "Fee, sats",
    numeric: true,
    optional: true,
    cell: (tx: Transaction | null) =>
      tx ? isCoinbase(tx) ? "" : formatSats(tx.fee ?? Number.NaN) : <Skel size="s" />,
  },
  {
    header: "Size, vB",
    numeric: true,
    optional: true,
    cell: (tx: Transaction | null) => {
      if (!tx) return <Skel size="s" />;
      const v = vsize(tx);
      return v === undefined ? MISSING : groupDigits(v);
    },
  },
  {
    header: "Value, BTC",
    numeric: true,
    cell: (tx: Transaction | null) =>
      tx ? <Amount sats={tx.vout.reduce((sum, o) => sum + o.value, 0)} unit={false} /> : <Skel />,
  },
];

/**
 * The block as a core sample: a frame the size of the weight limit, filled to how full the block
 * is. It holds the place of the transaction map, which needs a server route, so adding the map
 * later moves nothing.
 */
function CoreSample(props: { weight: number | undefined }) {
  const share = () => (props.weight === undefined ? undefined : fullness(props.weight));
  return (
    <figure class="block-core">
      <div class="block-core__frame">
        <span class="block-core__limit" aria-hidden="true">
          Weight limit
        </span>
        <Show when={share()} keyed>
          {(f) => (
            <div
              class="block-core__fill"
              classList={{ "block-core__fill--high": f > 0.85 }}
              style={{ "--fill": String(f) }}
            >
              <span class="block-core__level num">{percent(f)} full</span>
            </div>
          )}
        </Show>
      </div>
      <figcaption class="block-core__caption">
        {/* The sentence is always there, so it wraps the same while the figure loads. */}
        The block fills{" "}
        <Show when={share() !== undefined} fallback={<Skel size="s" />}>
          {percent(share() ?? 0)}
        </Show>{" "}
        of its weight limit. A map of its transactions by fee rate is not available yet.
      </figcaption>
    </figure>
  );
}
