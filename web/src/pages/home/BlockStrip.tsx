import {
  type Accessor,
  createEffect,
  createMemo,
  createSignal,
  For,
  Match,
  on,
  onCleanup,
  onMount,
  Show,
  Switch,
  untrack,
} from "solid-js";
import { FeeRate } from "../../components/FeeRate";
import { Link } from "../../components/Link";
import { Strata, strataLayers } from "../../components/Strata";
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
  vsizeFromWeight,
} from "../../lib/format";
import type { LiveStatus } from "../../lib/live";
import { paths } from "../../lib/network";
import { STRIP_MAX } from "./feeds";
import { LiveIndicator } from "./LiveIndicator";
import { type Motion, motion } from "./motion";
import type { TileFacts } from "./summaries";

const MAX_WEIGHT = 4_000_000;
/** The thinnest layer, as a share of a tile's strata box (about 45 px): 2 px, so one transaction shows. */
const TILE_LAYER_MIN = 0.045;

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
 * The block strip: the next block on the left of the "now" line, mined blocks on the right. When
 * the list changes it plays the new-block sequence (docs/DESIGN.md "Motion"): every tile is
 * measured, the list is swapped, then each tile glides from where it was (FLIP).
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
}) {
  const now = useNow();
  const [shown, setShown] = createSignal<Block[]>();
  const [announcement, setAnnouncement] = createSignal("");
  let scroller: HTMLDivElement | undefined;
  let lane: HTMLOListElement | undefined;
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

  /** Ends the sequence in progress where it stands, so a new one starts from what is on screen. */
  const interrupt = () => {
    for (const a of running) a.cancel();
    running.clear();
    for (const g of ghosts) g.remove();
    ghosts.clear();
    for (const t of timers) clearTimeout(t);
    timers.clear();
    for (const el of lane?.querySelectorAll(".home-tile--landing") ?? [])
      el.classList.remove("home-tile--landing", "home-tile--crossing");
  };

  createEffect(
    on(props.blocks, (list) => {
      const before = untrack(shown);
      const tip = list?.[0];
      if (before && tip && tip.height > (before[0]?.height ?? -1)) {
        const txs = `${groupDigits(tip.tx_count)} ${tip.tx_count === 1 ? "transaction" : "transactions"}`;
        setAnnouncement(`Block ${groupDigits(tip.height)} mined: ${txs}.`);
      }
      // Nothing to animate from, or nobody to see it: just show the new list.
      if (!list || !before || !lane || !next || document.hidden || !("animate" in lane)) {
        interrupt();
        setShown(list);
        return;
      }
      // First: where everything is now, mid-animation included.
      const first = new Map<string, DOMRect>();
      for (const el of lane.children) {
        const id = (el as HTMLElement).dataset["id"];
        if (id) first.set(id, el.getBoundingClientRect());
      }
      const from = next.getBoundingClientRect();
      const laneBox = lane.getBoundingClientRect();
      const keep = new Set(list.map((b) => b.id));
      const leaving = [...lane.children].filter((el) => {
        const id = (el as HTMLElement).dataset["id"];
        return id ? !keep.has(id) : false;
      });
      const ghostsOut = leaving.map((el) => {
        const g = el.cloneNode(true) as HTMLElement;
        const r = first.get(g.dataset["id"] ?? "");
        return { g, left: r ? r.left - laneBox.left : 0 };
      });
      const oldTip = before[0]?.height ?? Number.POSITIVE_INFINITY;
      interrupt();
      setShown(list);
      // Solid has updated the DOM by the time this runs, and the browser has not painted yet.
      queueMicrotask(() => sequence(first, from, ghostsOut, oldTip));
    }),
  );

  function sequence(
    first: Map<string, DOMRect>,
    from: DOMRect,
    out: { g: HTMLElement; left: number }[],
    oldTip: number,
  ) {
    if (!lane || !next) return;
    const m: Motion = motion();
    const tiles = [...lane.children] as HTMLElement[];
    const arriving = tiles.filter((el) => !first.has(el.dataset["id"] ?? ""));
    const crossing = arriving.filter((el) => Number(el.dataset["height"]) > oldTip);

    // Mined tiles shift along, the farthest first so none catches the one ahead.
    if (!m.reduced) {
      tiles.forEach((el, i) => {
        const was = first.get(el.dataset["id"] ?? "");
        if (!was) return;
        const dx = was.left - el.getBoundingClientRect().left;
        if (Math.abs(dx) < 1) return;
        play(el, [{ transform: `translateX(${dx}px)` }, { transform: "none" }], {
          duration: m.slow,
          easing: m.easeOut,
          delay: (tiles.length - 1 - i) * m.stagger,
          fill: "backwards",
        });
      });
    }

    // The tile that leaves fades where it stood.
    for (const { g, left } of out) {
      g.classList.add("home-tile--leaving");
      g.setAttribute("aria-hidden", "true");
      g.inert = true;
      g.style.setProperty("left", `${left}px`);
      lane.append(g);
      ghosts.add(g);
      const a = play(
        g,
        [{ opacity: 1 }, { opacity: 0, transform: m.reduced ? "none" : "translateX(24px)" }],
        {
          duration: m.slow || m.fade,
          easing: "ease-out",
          fill: "forwards",
        },
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

    // New blocks below the old tip (a reorg's replacements, or older ones filling in) fade in.
    for (const el of arriving) {
      if (crossing.includes(el)) continue;
      play(el, [{ opacity: 0 }, { opacity: 1 }], {
        duration: m.slow || m.fade,
        easing: "ease-out",
        fill: "backwards",
      });
    }
    if (crossing.length === 0) return;

    // The next block crosses the line on a shallow arc and lands with a small squash. Several at
    // once leave one after another, the one going farthest first.
    crossing.forEach((el, k) => {
      const delay = (crossing.length - 1 - k) * m.stagger * 3;
      if (m.reduced) {
        play(el, [{ opacity: 0 }, { opacity: 1 }], { duration: m.fade, delay, fill: "backwards" });
        return;
      }
      // Provisional while it travels; it sets, solid, at the top of its arc.
      el.classList.add("home-tile--landing", "home-tile--crossing");
      const set = setTimeout(
        () => {
          timers.delete(set);
          el.classList.remove("home-tile--crossing");
        },
        delay + m.block * 0.45,
      );
      timers.add(set);
      const dx = from.left - el.getBoundingClientRect().left;
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
      void arc.finished.then(
        () => el.classList.remove("home-tile--landing"),
        () => {
          // Cancelled: whoever cancelled it cleans up.
        },
      );
    });

    // The line flashes, and a new next block forms where the old one left.
    if (flash)
      play(flash, [{ opacity: 0 }, { opacity: 0.85, offset: 0.2 }, { opacity: 0 }], {
        duration: m.block || m.fade * 3,
      });
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

  // Phones scroll the strip; it starts with the "now" line in the middle.
  const center = () => {
    if (!scroller || !nowLine) return;
    scroller.scrollLeft = nowLine.offsetLeft - scroller.clientWidth / 2 + nowLine.offsetWidth / 2;
  };
  onMount(() => {
    center();
    let width = scroller?.clientWidth ?? 0;
    const ro = new ResizeObserver(() => {
      if (scroller && scroller.clientWidth !== width) {
        width = scroller.clientWidth;
        center();
      }
    });
    if (scroller) ro.observe(scroller);
    onCleanup(() => ro.disconnect());
  });
  onCleanup(interrupt);

  const sinceTip = () => {
    const tip = shown()?.[0];
    return tip ? formatRelative(tip.timestamp, now(), { suffix: false }) : undefined;
  };

  return (
    <section class="home-strip" aria-labelledby="strip-title">
      <div class="home-strip__head wrap">
        <h1 id="strip-title" class="home-strip__title">
          Blocks
        </h1>
        <p class="home-strip__legend">
          <span class="home-strip__key">Next block</span>
          <span class="home-strip__key home-strip__key--mined">Mined</span>
        </p>
        <LiveIndicator status={props.status} stale={props.stale} />
      </div>
      <div class="home-strip__scroller" ref={scroller}>
        <div class="home-strip__track">
          <div class="home-strip__upcoming">
            <p class="home-strip__pending">Blocks projected from the mempool will line up here.</p>
            <div class="home-tile home-tile--projected" ref={next}>
              <div class="home-tile__body">
                <span class="home-tile__title">Next block</span>
                <span class="home-tile__waiting">
                  <span class="home-tile__since num">{sinceTip() ?? ""}</span>
                  <span class="home-tile__since-label">{sinceTip() ? "since last block" : ""}</span>
                </span>
              </div>
              <span class="home-tile__pool" />
            </div>
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
                aria-busy={!shown()}
              >
                <Show
                  when={shown()}
                  fallback={
                    <For each={Array.from({ length: STRIP_MAX })}>{() => <SkeletonTile />}</For>
                  }
                >
                  <For each={shown()}>
                    {(b) => <MinedTile block={b} now={now} facts={props.facts(b.id)} />}
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

/** A tile's two fee lines as words, for its label: median, then range. */
export function tileFeeText(facts: TileFacts): string | undefined {
  if (typeof facts !== "object") return undefined;
  const r = facts.fee_rate;
  if (!r) return "only the coinbase, no fees";
  return `median fee ${formatFeeRate(r.median)} sat/vB, fee range ${formatFeeRate(r.min)} to ${formatFeeRate(r.max)} sat/vB`;
}

function MinedTile(props: { block: Block; now: Accessor<number>; facts: TileFacts }) {
  const b = () => props.block;
  const full = () => Math.min(1, Math.max(0, b().weight / MAX_WEIGHT));
  const summary = () => (typeof props.facts === "object" ? props.facts : undefined);
  const rates = () => summary()?.fee_rate ?? undefined;
  const layers = createMemo(() => {
    const s = summary();
    return s ? strataLayers(s.fee_bands, full(), TILE_LAYER_MIN) : [];
  });
  // Facts that arrive after the tile is drawn settle in; facts it was drawn with are simply there.
  const settle = untrack(() => props.facts === undefined);
  const label = () =>
    [
      `Block ${groupDigits(b().height)}`,
      formatRelative(b().timestamp, props.now()),
      `${groupDigits(b().tx_count)} transactions`,
      tileFeeText(props.facts),
      fullness(b().weight),
    ]
      .filter(Boolean)
      .join(", ");
  return (
    <li class="home-tile home-tile--mined" data-id={b().id} data-height={b().height}>
      <Link class="home-tile__body" href={paths.block(b().id)} aria-label={label()}>
        <span class="home-tile__title num">{groupDigits(b().height)}</span>
        <span class="home-tile__fee num">
          <Switch fallback={<span class="home-tile__bone home-tile__bone--line" />}>
            <Match when={rates()}>{(r) => <FeeRate rate={r().median} />}</Match>
            <Match when={summary()}>No fees</Match>
            <Match when={props.facts === "failed"}>{formatSize(vsizeFromWeight(b().weight))}</Match>
          </Switch>
        </span>
        <span class="home-tile__range num">
          <Switch
            fallback={<span class="home-tile__bone home-tile__bone--line home-tile__bone--short" />}
          >
            <Match when={rates()}>
              {(r) => `${formatFeeRate(r().min)} – ${formatFeeRate(r().max)}`}
            </Match>
            <Match when={summary()}>Coinbase only</Match>
            <Match when={props.facts === "failed"}>{fullness(b().weight)}</Match>
          </Switch>
        </span>
        <span class="home-tile__strata" aria-hidden="true">
          {/* The neutral gauge: how full the block is, until its layers arrive and settle over it. */}
          <Show when={layers().length === 0 || settle}>
            <span
              class="home-tile__fill"
              classList={{ "home-tile__fill--gone": layers().length > 0 }}
              style={{ "--fill": String(full()) }}
            />
          </Show>
          <Show when={layers().length > 0}>
            <span class="home-tile__layers">
              <Strata layers={layers()} settle={settle} />
            </span>
          </Show>
        </span>
        <span class="home-tile__foot">
          <span class="num">{groupDigits(b().tx_count)} tx</span>
          <time class="num" datetime={isoTime(b().timestamp)} title={formatUtc(b().timestamp)}>
            {tileAge(b().timestamp, props.now())}
          </time>
        </span>
      </Link>
      {/* The miner's name goes here; the line is kept so it lands without moving anything. */}
      <span class="home-tile__pool" />
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
