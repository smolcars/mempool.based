import {
  type Accessor,
  createEffect,
  createMemo,
  createSignal,
  For,
  Match,
  onCleanup,
  onSettled,
  Show,
  Switch,
  untrack,
} from "solid-js";
import { Amount } from "../../components/Amount";
import { FeeRate } from "../../components/FeeRate";
import { Link } from "../../components/Link";
import { type Layer, Strata, strataLayers } from "../../components/Strata";
import { ErrorState } from "../../components/States";
import { useNow } from "../../lib/clock";
import type { Block } from "../../lib/esplora";
import {
  formatFeeRate,
  formatRelative,
  formatSize,
  formatUtc,
  groupDigits,
  isoTime,
} from "../../lib/format";
import type { LiveStatus } from "../../lib/live";
import type { ProjectedBlock } from "../../lib/projected-topic";
import { type Network, paths } from "../../lib/network";
import { STRIP_MAX } from "./feeds";
import { LiveIndicator } from "./LiveIndicator";
import { type Motion, motion } from "./motion";
import {
  EMPTY_MODEL,
  type ProjectedView,
  type ProjectionModel,
  projectedView,
  provisionalFor,
  releaseIn,
  sameTiles,
} from "./projection";
import { ProjectedTile, projectedLayers, sameLayers, TILE_LAYER_MIN } from "./ProjectedTile";
import type { TileFacts } from "./summaries";

const MAX_WEIGHT = 4_000_000;
/** Projected tiles on screen, not the ones fading out. */
const PROJECTED = ".home-tile--projected[data-key]:not(.home-tile--leaving)";

/** How full a block is, as tile text: `99.9% full`, `under 1% full`. */
export function fullness(weight: number): string {
  const pct = (Math.min(MAX_WEIGHT, Math.max(0, weight)) / MAX_WEIGHT) * 100;
  if (pct < 1) return "under 1% full";
  return `${Math.round(pct * 10) / 10}% full`;
}

/** A tile's age, short enough for its foot: `just now`, `42 s`, `13 min`, then whole hours and days. */
export function tileAge(time: number, now: number): string {
  const s = now - time;
  if (s < 3600) return formatRelative(time, now, { suffix: false });
  if (s < 48 * 3600) return `${Math.floor(s / 3600)} h`;
  return `${Math.floor(s / 86_400)} d`;
}

/**
 * The block strip: projected blocks left of the "now" line, mined blocks right of it. Whenever the
 * tiles change places (a block is mined, a burst, a reorg, the projection catching up) it plays
 * the new-block sequence (docs/DESIGN.md "Motion"): every tile is measured, both lanes are
 * swapped in one go, then each tile glides from where it was (FLIP), and a mined block that was
 * projected crosses the line from its projected tile. A change of figures alone moves nothing.
 */
export function BlockStrip(props: {
  blocks: Accessor<Block[] | undefined>;
  error: Accessor<unknown>;
  retry: () => void;
  status: Accessor<LiveStatus>;
  /** The node is not answering the page's requests, though the socket is up. */
  stale: Accessor<boolean>;
  /** A mined block's fee facts, by hash. */
  facts: (id: string) => TileFacts;
  projection: Accessor<ProjectionModel>;
  network: Network;
}) {
  const now = useNow();
  const [shown, setShown] = createSignal<Block[]>();
  const [view, setView] = createSignal<ProjectedView>(projectedView(EMPTY_MODEL, undefined, 0));
  const [announcement, setAnnouncement] = createSignal("");
  // A held projection is released by time, not by a message: this ticks when it is due.
  const [tick, setTick] = createSignal(0);
  /** Blocks crossing the line: what each was projected to be, until it lands. */
  const crossings = new Map<string, Crossing>();
  /** Lands each crossing still in the air; the sequence calls one as its tile touches down. */
  const landers = new Map<string, () => void>();
  const landAll = () => {
    for (const land of landers.values()) land();
    landers.clear();
  };
  /** Whether projected tiles drawn now settle their layers in by themselves (see `sequence`). */
  let settleNew = false;
  let scroller: HTMLDivElement | undefined;
  let lane: HTMLOListElement | undefined;
  let upcoming: HTMLDivElement | undefined;
  let next: HTMLDivElement | undefined;
  let flash: HTMLSpanElement | undefined;
  let nowLine: HTMLDivElement | undefined;
  const running = new Set<Animation>();
  const ghosts = new Set<Element>();
  const timers = new Set<ReturnType<typeof setTimeout>>();

  const play = (el: Element, keyframes: Keyframe[], options: KeyframeAnimationOptions) => {
    const a = el.animate(keyframes, options);
    running.add(a);
    void a.finished.then(
      () => running.delete(a),
      () => running.delete(a),
    );
    return a;
  };
  const later = (ms: number, f: () => void) => {
    const t = setTimeout(() => {
      timers.delete(t);
      f();
    }, ms);
    timers.add(t);
  };

  /** Ends the sequence in progress where it stands, so a new one starts from what is on screen. */
  const interrupt = () => {
    // Whatever was in the air lands where the next sequence starts it from: settled.
    landAll();
    for (const a of running) a.cancel();
    running.clear();
    for (const g of ghosts) g.remove();
    ghosts.clear();
    for (const t of timers) clearTimeout(t);
    timers.clear();
    for (const el of lane?.querySelectorAll<HTMLElement>(".home-tile--landing") ?? []) {
      el.classList.remove("home-tile--landing", "home-tile--crossing");
    }
  };

  const target = createMemo(() => {
    tick();
    const blocks = props.blocks();
    return { blocks, view: projectedView(props.projection(), blocks, Date.now()) };
  });
  createEffect(
    () => releaseIn(props.projection(), props.blocks(), Date.now()),
    (wait) => {
      if (wait === undefined) return undefined;
      const t = setTimeout(() => setTick((n) => n + 1), wait + 50);
      return () => clearTimeout(t);
    },
  );

  createEffect(target, ({ blocks: list, view: nextView }) => {
    const before = shown();
    const beforeView = view();
    const minedSame = sameIds(before, list);
    // The same tiles in the same places: the figures change where they stand.
    if (minedSame && sameTiles(beforeView, nextView)) {
      setView(nextView);
      return;
    }
    const tip = list?.[0];
    const oldTip = before?.[0]?.height ?? Number.POSITIVE_INFINITY;
    if (before && list && tip && tip.height > oldTip) {
      const txs = `${groupDigits(tip.tx_count)} ${tip.tx_count === 1 ? "transaction" : "transactions"}`;
      setAnnouncement(`Block ${groupDigits(tip.height)} mined: ${txs}.`);
    }
    // What the new blocks were projected to be, shown until each lands.
    const fresh: [string, ProjectedBlock][] = [];
    if (before && list && !document.hidden) {
      const model = props.projection();
      for (const b of list) {
        if (b.height <= oldTip) break;
        const p = provisionalFor(model, list, b);
        if (p) fresh.push([b.id, p]);
      }
    }
    const fly = () => {
      for (const [id, guess] of fresh) {
        const [landed, setLanded] = createSignal(false);
        crossings.set(id, { guess, landed });
        landers.set(id, () => setLanded(true));
      }
    };
    const show = () => {
      setShown(list);
      setView(nextView);
    };
    // Nothing to animate from, or nobody to see it: just show the new state.
    if (!list || !before || !lane || !upcoming || document.hidden || !("animate" in lane)) {
      interrupt();
      settleNew = !document.hidden;
      show();
      return;
    }
    // First: where everything is now, mid-animation included.
    const first = new Map<string, DOMRect>();
    const tiles = [...lane.children, ...upcoming.querySelectorAll(PROJECTED)].filter(
      (el) => !el.classList.contains("home-tile--leaving"),
    );
    for (const el of tiles as HTMLElement[]) {
      const id = el.dataset["id"] ?? (el.dataset["key"] ? `p${el.dataset["key"]}` : undefined);
      if (id) first.set(id, el.getBoundingClientRect());
    }
    // Where a crossing block leaves from when it had no tile of its own: the next block's place.
    const from = (
      upcoming.querySelector(".home-tile--projected") ?? upcoming
    ).getBoundingClientRect();
    const keep = new Set([
      ...list.map((b) => b.id),
      ...nextView.tiles.map((t) => `p${t.key}`),
      // A projected tile that becomes a mined block leaves as that block, not as a ghost.
      ...list.map((b) => `p${b.height}`),
    ]);
    const out = (tiles as HTMLElement[])
      .filter((el) => {
        const id = el.dataset["id"] ?? `p${el.dataset["key"] ?? ""}`;
        return !keep.has(id);
      })
      .map((el) => {
        const parent = el.parentElement;
        const g = el.cloneNode(true) as HTMLElement;
        const r = el.getBoundingClientRect();
        const box = parent?.getBoundingClientRect();
        return { g, parent, left: box ? r.left - box.left : 0 };
      });
    interrupt();
    fly();
    // A tile that forms behind a crossing block settles in time with the crossing, below.
    const crossingNow = !!tip && tip.height > oldTip;
    settleNew = !crossingNow;
    show();
    // Solid has updated the DOM by the time this runs, and the browser has not painted yet.
    queueMicrotask(() => sequence(first, from, out, oldTip));
  });

  function sequence(
    first: Map<string, DOMRect>,
    from: DOMRect,
    out: { g: HTMLElement; parent: HTMLElement | null; left: number }[],
    oldTip: number,
  ) {
    if (!lane || !upcoming) {
      landAll();
      return;
    }
    const m: Motion = motion();
    const mined = [...lane.children] as HTMLElement[];
    const projected = [...upcoming.querySelectorAll<HTMLElement>(PROJECTED)];
    const idOf = (el: HTMLElement) => el.dataset["id"] ?? `p${el.dataset["key"] ?? ""}`;
    const arriving = mined.filter((el) => !first.has(idOf(el)));
    const crossing = arriving.filter((el) => Number(el.dataset["height"]) > oldTip);
    const glide = (el: HTMLElement, order: number) => {
      const was = first.get(idOf(el));
      if (!was || m.reduced) return;
      const dx = was.left - el.getBoundingClientRect().left;
      if (Math.abs(dx) < 1) return;
      play(el, [{ transform: `translateX(${dx}px)` }, { transform: "none" }], {
        duration: m.slow,
        easing: m.easeOut,
        delay: order * m.stagger,
        fill: "backwards",
      });
    };

    // Leaders move first: mined tiles from the farthest, projected tiles from the nearest.
    mined.forEach((el, i) => glide(el, mined.length - 1 - i));
    projected.forEach((el, i) => glide(el, 2 + i));

    // Tiles that leave fade where they stood.
    for (const { g, parent, left } of out) {
      if (!parent) continue;
      g.classList.add("home-tile--leaving");
      g.setAttribute("aria-hidden", "true");
      g.inert = true;
      g.style.setProperty("left", `${left}px`);
      parent.append(g);
      ghosts.add(g);
      const a = play(
        g,
        [{ opacity: 1 }, { opacity: 0, transform: m.reduced ? "none" : "translateX(24px)" }],
        { duration: m.slow || m.fade, easing: "ease-out", fill: "forwards" },
      );
      void a.finished.then(
        () => {
          g.remove();
          ghosts.delete(g);
        },
        () => {
          // Cancelled: whoever cancelled it cleans up.
        },
      );
    }

    // New projected tiles form at the far end once the crossing is under way, their layers
    // dropping in one at a time, cheapest first, like sediment.
    for (const el of projected) {
      if (first.has(idOf(el))) continue;
      if (crossing.length > 0 && !m.reduced) {
        el.querySelectorAll(".strata__layer").forEach((layer, i) =>
          play(
            layer,
            [
              { opacity: 0, transform: "translateY(-18px)" },
              { opacity: 1, transform: "none" },
            ],
            {
              duration: m.slow,
              easing: m.easeSettle,
              delay: m.block * 0.4 + i * m.stagger,
              fill: "backwards",
            },
          ),
        );
      }
      play(
        el,
        [
          { opacity: 0, transform: m.reduced ? "none" : "translateX(-28px)" },
          { opacity: 1, transform: "none" },
        ],
        {
          duration: m.slow || m.fade,
          easing: m.easeOut,
          delay: crossing.length > 0 ? m.block * 0.35 : 0,
          fill: "backwards",
        },
      );
    }

    // New blocks below the old tip (a reorg's replacements, or older ones filling in) fade in.
    for (const el of arriving) {
      if (crossing.includes(el)) continue;
      play(el, [{ opacity: 0 }, { opacity: 1 }], {
        duration: m.slow || m.fade,
        easing: "ease-out",
        fill: "backwards",
      });
    }
    if (crossing.length === 0) {
      landAll();
      return;
    }

    // Each new block crosses the line from its projected tile on a shallow arc and lands with a
    // small squash. Several at once leave one after another, the one going farthest first.
    crossing.forEach((el, k) => {
      const delay = (crossing.length - 1 - k) * m.stagger * 3;
      const id = el.dataset["id"] ?? "";
      const land = () => {
        landers.get(id)?.();
        landers.delete(id);
      };
      el.classList.add("home-tile--landing");
      if (m.reduced) {
        play(el, [{ opacity: 0 }, { opacity: 1 }], { duration: m.fade, delay, fill: "backwards" });
        later(delay + m.fade, land);
        // Nothing travels, but the accent still marks the new block for about as long.
        later(delay + m.fade * 6, () => el.classList.remove("home-tile--landing"));
        return;
      }
      // Provisional while it travels; it sets, solid, at the top of its arc.
      el.classList.add("home-tile--crossing");
      later(delay + m.block * 0.45, () => el.classList.remove("home-tile--crossing"));
      const start = first.get(`p${el.dataset["height"] ?? ""}`) ?? from;
      const dx = start.left - el.getBoundingClientRect().left;
      const arc = play(
        el,
        [
          { transform: `translateX(${dx}px)` },
          { transform: `translate(${dx * 0.45}px, -14px) scale(1.05)`, offset: 0.45 },
          { transform: "none" },
        ],
        { duration: m.block, easing: m.easeInOut, delay, fill: "backwards" },
      );
      const body = el.querySelector(".home-tile__body");
      if (body) {
        play(
          body,
          [
            { transform: "scaleY(1)" },
            { transform: "scaleY(1)", offset: 0.82 },
            { transform: "scaleY(0.94)", offset: 0.9 },
            { transform: "scaleY(1)" },
          ],
          { duration: m.block, easing: m.easeSettle, delay },
        );
      }
      // Its projected figures and layers give way to its own as it touches down.
      later(delay + m.block * 0.82, land);
      void arc.finished.then(
        () => el.classList.remove("home-tile--landing"),
        () => {
          // Cancelled: whoever cancelled it cleans up.
        },
      );
    });

    // The line flashes, and the placeholder for the next block, if that is what shows, re-forms.
    if (flash)
      play(flash, [{ opacity: 0 }, { opacity: 0.85, offset: 0.2 }, { opacity: 0 }], {
        duration: m.block || m.fade * 3,
      });
    if (next?.isConnected) {
      play(
        next,
        [
          { opacity: 0, transform: m.reduced ? "none" : "translateX(-28px)" },
          { opacity: 1, transform: "none" },
        ],
        {
          duration: m.slow || m.fade,
          easing: m.easeOut,
          delay: m.block * 0.35,
          fill: "backwards",
        },
      );
    }
  }

  // Phones scroll the strip; it starts with the "now" line in the middle.
  const center = () => {
    if (!scroller || !nowLine) return;
    scroller.scrollLeft = nowLine.offsetLeft - scroller.clientWidth / 2 + nowLine.offsetWidth / 2;
  };
  onSettled(() => {
    center();
    let width = scroller?.clientWidth ?? 0;
    const ro = new ResizeObserver(() => {
      if (scroller && scroller.clientWidth !== width) {
        width = scroller.clientWidth;
        center();
      }
    });
    if (scroller) ro.observe(scroller);
    return () => ro.disconnect();
  });
  onCleanup(interrupt);

  const sinceTip = () => {
    const tip = shown()?.[0];
    return tip ? formatRelative(tip.timestamp, now(), { suffix: false }) : undefined;
  };
  const keys = createMemo(() => view().tiles.map((t) => t.key));
  /** A crossing is for the tile drawn for it, once: a refetch redraws it settled. */
  const take = (id: string) => {
    const c = crossings.get(id);
    crossings.delete(id);
    return c;
  };
  const tileAt = (key: number) => () => view().tiles.find((t) => t.key === key);
  const note = () => {
    const v = view();
    if (v.mode === "off") return "This server does not track the mempool on this network.";
    if (v.stale) return "The last projection before the server lost touch with its node.";
    const [first, second] = v.tiles;
    if (!first) return "";
    if (first.txs === 0) return "Nothing is waiting. The next block would hold only its coinbase.";
    return second ? "" : "Everything waiting fits in the next block.";
  };

  return (
    <section class="home-strip" aria-labelledby="strip-title">
      <div class="home-strip__head wrap">
        <h1 id="strip-title" class="home-strip__title">
          Blocks
        </h1>
        <p class="home-strip__legend">
          <span class="home-strip__key">
            {view().mode === "off" ? "Next block" : "Projected from the mempool"}
          </span>
          <span class="home-strip__key home-strip__key--mined">Mined</span>
        </p>
        <LiveIndicator status={props.status} stale={props.stale} />
      </div>
      <div class="home-strip__scroller" ref={scroller}>
        <div class="home-strip__track">
          <div class="home-strip__upcoming" ref={upcoming}>
            <p class="home-strip__pending">{note()}</p>
            <Show
              when={keys().length > 0}
              fallback={
                <div class="home-tile home-tile--projected" ref={next}>
                  <div class="home-tile__body">
                    <span class="home-tile__title">Next block</span>
                    <span class="home-tile__waiting">
                      <span class="home-tile__since num">{sinceTip() ?? ""}</span>
                      <span class="home-tile__since-label">
                        {sinceTip() ? "since last block" : ""}
                      </span>
                    </span>
                  </div>
                  <span class="home-tile__pool" />
                </div>
              }
            >
              <ol class="home-strip__projected" aria-label="Projected blocks, next first">
                <For each={keys()}>
                  {(key) => (
                    <ProjectedTile tile={tileAt(key)} network={props.network} settle={settleNew} />
                  )}
                </For>
              </ol>
            </Show>
          </div>
          <div class="home-strip__now" ref={nowLine} aria-hidden="true">
            <span class="home-strip__flash" ref={flash} />
            <span class="home-strip__now-label">now</span>
          </div>
          <div class="home-strip__mined">
            <Show
              when={shown() ?? props.error() === undefined}
              fallback={
                <div class="home-strip__error">
                  <ErrorState
                    title="Could not load the latest blocks"
                    error={props.error()}
                    retry={props.retry}
                  />
                </div>
              }
            >
              <ol
                class="home-strip__lane"
                ref={lane}
                aria-label="Mined blocks, newest first"
                aria-busy={shown() ? "false" : "true"}
              >
                <Show
                  when={shown()}
                  fallback={
                    <For each={Array.from({ length: STRIP_MAX })}>{() => <SkeletonTile />}</For>
                  }
                >
                  <For each={shown()}>
                    {(b) => (
                      <MinedTile
                        block={b}
                        now={now}
                        facts={props.facts(b.id)}
                        crossing={take(b.id)}
                      />
                    )}
                  </For>
                </Show>
              </ol>
            </Show>
          </div>
        </div>
      </div>
      <p class="visually-hidden" aria-live="polite">
        {announcement()}
      </p>
    </section>
  );
}

function sameIds(a: readonly Block[] | undefined, b: readonly Block[] | undefined): boolean {
  if (!a || !b) return a === b;
  return a.length === b.length && a.every((x, i) => x.id === b[i]?.id);
}

/** A tile's two fee lines as words, for its label: median, then range. */
export function tileFeeText(facts: TileFacts): string | undefined {
  if (typeof facts !== "object") return undefined;
  const r = facts.fee_rate;
  if (!r) return "only the coinbase, no fees";
  return `median fee ${formatFeeRate(r.median)} sat/vB, fee range ${formatFeeRate(r.min)} to ${formatFeeRate(r.max)} sat/vB`;
}

/** A block crossing the line: what it was projected to be, and whether it has landed. */
interface Crossing {
  guess: ProjectedBlock;
  landed: Accessor<boolean>;
}

function MinedTile(props: {
  block: Block;
  now: Accessor<number>;
  facts: TileFacts;
  /** When it is crossing the line: what it was projected to be, and whether it has landed. */
  crossing: Crossing | undefined;
}) {
  const b = () => props.block;
  const full = () => Math.min(1, Math.max(0, b().weight / MAX_WEIGHT));
  const crossing = untrack(() => props.crossing);
  const projected = crossing?.guess;
  const guess = projected ? projectedLayers(projected) : undefined;
  // A block crossing the line shows what it was projected to be until it lands, however soon its
  // own facts arrive; then its layers grow out of the projected ones.
  const facts = () => (!crossing || crossing.landed() ? props.facts : undefined);
  const summary = () => {
    const f = facts();
    return typeof f === "object" ? f : undefined;
  };
  /** Still showing what it was projected to be. */
  const guessing = () => !!projected && facts() === undefined;
  // Facts that arrive after the tile is drawn settle in, or grow out of the projected layers;
  // facts it was drawn with are simply there.
  const settle = untrack(() => props.facts === undefined) && !guess;
  const rates = () => summary()?.fee_rate ?? undefined;
  // Any summary arriving re-reads every tile's facts; unchanged layers keep their elements, so a
  // tile that settled in once does not settle again.
  const layers = createMemo<Layer[]>(
    () => {
      const s = summary();
      return s ? strataLayers(s.fee_bands, full(), TILE_LAYER_MIN) : [];
    },
    { equals: sameLayers },
  );
  const miner = () => summary()?.miner?.name;
  const label = () =>
    [
      `Block ${groupDigits(b().height)}`,
      miner() && `mined by ${miner()}`,
      formatRelative(b().timestamp, props.now()),
      `${groupDigits(b().tx_count)} ${b().tx_count === 1 ? "transaction" : "transactions"}`,
      tileFeeText(facts()),
      fullness(b().weight),
    ]
      .filter(Boolean)
      .join(", ");
  return (
    <li
      class={["home-tile home-tile--mined", { "home-tile--guessing": guessing() }]}

      data-id={b().id}
      data-height={b().height}
    >
      <Link class="home-tile__body" href={paths.block(b().id)} aria-label={label()}>
        <span class="home-tile__title num">{groupDigits(b().height)}</span>
        <span class="home-tile__fee num">
          <Switch fallback={<span class="home-tile__bone home-tile__bone--line" />}>
            <Match when={rates()}>{(r) => <FeeRate rate={r().median} />}</Match>
            <Match when={guessing() && projected}>
              {(p) => <FeeRate rate={p().medianRate} approx />}
            </Match>
            <Match when={summary()}>No fees</Match>
            <Match when={facts() === "failed"}>{formatSize(b().size, "B")}</Match>
          </Switch>
        </span>
        <span class="home-tile__range num">
          <Switch
            fallback={<span class="home-tile__bone home-tile__bone--line home-tile__bone--short" />}
          >
            <Match when={rates()}>
              {(r) => <>{`${formatFeeRate(r().min)} – ${formatFeeRate(r().max)}`}</>}
            </Match>
            <Match when={guessing() && projected}>
              {(p) => <>{`${formatFeeRate(p().minRate)} – ${formatFeeRate(p().maxRate)}`}</>}
            </Match>
            <Match when={summary()}>Coinbase only</Match>
            <Match when={facts() === "failed"}>{fullness(b().weight)}</Match>
          </Switch>
        </span>
        <span class="home-tile__fees">
          <Switch>
            <Match when={summary()}>{(f) => <Amount sats={f().fees} short />}</Match>
            <Match when={guessing() && projected}>
              {(p) => <Amount sats={p().fees} short approx />}
            </Match>
          </Switch>
        </span>
        <span class="home-tile__strata" aria-hidden="true">
          {/* The neutral gauge: how full the block is, until its layers arrive and settle over it. */}
          <Show when={(layers().length === 0 && !guessing()) || settle}>
            <span
              class={["home-tile__fill", { "home-tile__fill--gone": layers().length > 0 }]}

              style={{ "--fill": String(full()) }}
            />
          </Show>
          {/* What it was projected to be, fading as the real layers grow out of it. */}
          <Show when={guess}>
            {(g) => (
              <span
                class={[
                  "home-tile__layers home-tile__guess",
                  { "home-tile__guess--gone": !guessing() },
                ]}
              >
                <Strata layers={g()} />
              </span>
            )}
          </Show>
          <Show when={layers().length > 0}>
            <span class="home-tile__layers">
              <Strata layers={layers()} settle={settle} from={guess} />
            </span>
          </Show>
        </span>
        <span class="home-tile__foot">
          <span class="num">
            {guessing() && projected
              ? `~${groupDigits(projected.txs)} tx`
              : `${groupDigits(b().tx_count)} tx`}
          </span>
          <time class="num" datetime={isoTime(b().timestamp)} title={formatUtc(b().timestamp)}>
            {tileAge(b().timestamp, props.now())}
          </time>
        </span>
      </Link>
      {/* Its line is always there, so the name lands without moving anything; the link says it. */}
      <span class="home-tile__pool" aria-hidden="true" title={miner()}>
        {miner()}
      </span>
    </li>
  );
}

function SkeletonTile() {
  return (
    <li class="home-tile home-tile--skeleton" aria-hidden="true">
      <span class="home-tile__body">
        <span class="home-tile__bone home-tile__bone--title" />
        <span class="home-tile__bone" />
        <span class="home-tile__bone home-tile__bone--short" />
      </span>
      <span class="home-tile__pool" />
    </li>
  );
}
