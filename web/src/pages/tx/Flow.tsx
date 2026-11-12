import {
  createEffect,
  createSignal,
  For,
  Index,
  type JSX,
  on,
  onCleanup,
  onMount,
  Show,
} from "solid-js";
import { Amount } from "../../components/Amount";
import { FeeRate } from "../../components/FeeRate";
import { Hash } from "../../components/Hash";
import { Link } from "../../components/Link";
import type { Outspend, Transaction, Vout } from "../../lib/esplora";
import { feeColor, groupDigits } from "../../lib/format";
import { paths } from "../../lib/network";
import { classify, isText, opReturnData, printable } from "./script";
import { cssMs, isCoinbase, MORE_STEP, shownCount, totalIn, totalOut, vsize } from "./tx";

export type Side = "in" | "out";
export interface Selection {
  side: Side;
  index: number;
}

/** OP_RETURN text shown on a flow card is cut here; the inspector has the rest. */
const CARD_TEXT_CHARS = 40;

/** What a card names when there is no address: the script type, with an OP_RETURN's text if it has some. */
export function scriptLabel(out: Vout): string {
  const c = classify(out.scriptpubkey);
  if (c.kind !== "op_return") return c.label;
  const data = opReturnData(out.scriptpubkey);
  if (!isText(data))
    return data.length > 0 ? `OP_RETURN, ${groupDigits(data.length)} bytes` : "OP_RETURN";
  const p = printable(data, CARD_TEXT_CHARS);
  return `OP_RETURN “${p.text}${p.chars < printable(data).chars ? "…" : ""}”`;
}

interface Ribbon {
  d: string;
  kind: "in" | "out" | "fee" | "more";
  key: string;
}

const tokenMs = (name: string) =>
  cssMs(getComputedStyle(document.documentElement).getPropertyValue(name));

/**
 * Inputs on the left, outputs on the right, joined by ribbons as thick as the value they carry.
 * Shows FIRST_SHOWN of each side, then MORE_STEP more per press; hidden items travel as one ribbon.
 */
export function Flow(props: {
  tx: Transaction;
  outspends: () => Outspend[] | null | undefined;
  selected: Selection | null;
  onSelect: (s: Selection) => void;
}) {
  const [inPresses, setInPresses] = createSignal(0);
  const [outPresses, setOutPresses] = createSignal(0);
  const vin = () => props.tx.vin ?? [];
  const inShown = () => shownCount(vin().length, inPresses());
  const outShown = () => shownCount(props.tx.vout.length, outPresses());
  const coinbase = () => isCoinbase(props.tx);
  const fee = () => (coinbase() ? 0 : (props.tx.fee ?? 0));
  const rate = () => {
    const v = vsize(props.tx);
    return v !== undefined && v !== 0 ? fee() / v : 0;
  };
  const hiddenIn = () =>
    vin()
      .slice(inShown())
      .reduce((a, i) => a + (i.prevout?.value ?? 0), 0);
  const hiddenOut = () => props.tx.vout.slice(outShown()).reduce((a, o) => a + o.value, 0);

  let flowEl: HTMLDivElement | undefined;
  let svgEl: SVGSVGElement | undefined;
  let inCol: HTMLUListElement | undefined;
  let outCol: HTMLUListElement | undefined;
  const [ribbons, setRibbons] = createSignal<Ribbon[]>([]);
  const [trunk, setTrunk] = createSignal({ x: 0, y: 0, h: 0 });
  const [view, setView] = createSignal("0 0 0 0");

  // Ribbon geometry from where the cards actually sit. One layout read per draw.
  const draw = () => {
    if (!svgEl || !inCol || !outCol) return;
    const box = svgEl.getBoundingClientRect();
    const W = box.width;
    const H = box.height;
    if (W === 0 || H === 0) return;
    const total = Math.max(totalIn(props.tx), totalOut(props.tx) + fee(), 1);
    const T = Math.min(H * 0.8, 96);
    const mid = W / 2;
    const thicknesses = (col: HTMLUListElement) =>
      ([...col.children] as HTMLElement[]).map((el) =>
        Math.max(1.5, (Number(el.dataset.value) / total) * T),
      );
    const tIn = thicknesses(inCol);
    const tOut = thicknesses(outCol);
    const sumOf = (xs: number[]) => xs.reduce((a, b) => a + b, 0);
    const side = (col: HTMLUListElement, thick: number[], toRight: boolean): Ribbon[] => {
      const items = [...col.children] as HTMLElement[];
      let y = H / 2 - sumOf(thick) / 2;
      const c = (toRight ? 1 : -1) * W * 0.45;
      return items.map((el, k) => {
        const r = el.getBoundingClientRect();
        const cy = r.top + r.height / 2 - box.top;
        const th = thick[k] ?? 1.5;
        const a = { x: toRight ? 0 : W, y0: cy - th / 2, y1: cy + th / 2 };
        const b = { x: mid, y0: y, y1: y + th };
        y += th;
        const f = (n: number) => n.toFixed(1);
        return {
          d: `M${f(a.x)},${f(a.y0)} C${f(a.x + c)},${f(a.y0)} ${f(b.x - c)},${f(b.y0)} ${f(b.x)},${f(b.y0)} L${f(b.x)},${f(b.y1)} C${f(b.x - c)},${f(b.y1)} ${f(a.x + c)},${f(a.y1)} ${f(a.x)},${f(a.y1)} Z`,
          kind: (el.dataset.kind as Ribbon["kind"]) ?? (toRight ? "in" : "out"),
          key: `${toRight ? "i" : "o"}${el.dataset.key ?? k}`,
        };
      });
    };
    const left = side(inCol, tIn, true);
    const right = side(outCol, tOut, false);
    const span = Math.max(sumOf(tIn), sumOf(tOut));
    setView(`0 0 ${W} ${H}`);
    setTrunk({ x: mid, y: H / 2 - span / 2, h: span });
    setRibbons([...left, ...right]);
  };

  let first = true;
  const animate = () => {
    if (!first || !svgEl || matchMedia("(prefers-reduced-motion: reduce)").matches) return;
    first = false;
    const d = tokenMs("--dur-slow");
    const s = tokenMs("--stagger");
    const ease = getComputedStyle(document.documentElement).getPropertyValue("--ease-out").trim();
    if (!d) return;
    // Value flows in from the left, then out to the right; the stagger stops after a dozen.
    [...svgEl.querySelectorAll<SVGPathElement>(".tx-ribbon")].forEach((p, k) => {
      const out = p.dataset.side === "out";
      p.animate(
        [
          { opacity: 0, transform: "translateX(-8px)" },
          { opacity: 1, transform: "none" },
        ],
        {
          duration: d,
          delay: (out ? d * 0.5 : 0) + Math.min(k, 12) * s,
          easing: ease || "ease-out",
          fill: "backwards",
        },
      );
    });
  };

  onMount(() => {
    const ro = new ResizeObserver(() => draw());
    if (flowEl) ro.observe(flowEl);
    onCleanup(() => ro.disconnect());
    requestAnimationFrame(() => {
      draw();
      requestAnimationFrame(animate);
    });
  });
  // Cards added or relabelled: redraw after they are laid out.
  createEffect(
    on([inShown, outShown, props.outspends], () => requestAnimationFrame(draw), { defer: true }),
  );

  const isSel = (side: Side, index: number) =>
    props.selected?.side === side && props.selected.index === index;

  const openButton = (side: Side, index: number) => (
    <button
      type="button"
      class="tx-flow__open"
      aria-expanded={isSel(side, index)}
      aria-controls="tx-inspector"
      aria-label={`Details of ${side === "in" ? "input" : "output"} ${index}`}
      onClick={() => props.onSelect({ side, index })}
    >
      <svg viewBox="0 0 16 16" aria-hidden="true">
        <path d="M4 6.5l4 4 4-4" />
      </svg>
    </button>
  );

  const more = (side: Side, hidden: number, value: number, press: () => void): JSX.Element => (
    <li
      class="tx-flow__item tx-flow__item--more"
      data-value={value}
      data-kind="more"
      data-key="more"
    >
      <button type="button" class="tx-flow__more" onClick={press}>
        Show {groupDigits(Math.min(hidden, MORE_STEP))} more
      </button>
      <span class="tx-flow__meta">
        {groupDigits(hidden)} more {side === "in" ? "inputs" : "outputs"}, <Amount sats={value} />
      </span>
    </li>
  );

  return (
    <div class="tx-flow" ref={flowEl}>
      <ul class="tx-flow__col tx-flow__col--in" aria-label="Inputs" ref={inCol}>
        <Show
          when={!coinbase()}
          fallback={
            <li
              class="tx-flow__item"
              classList={{ "is-selected": isSel("in", 0) }}
              data-value={totalOut(props.tx)}
              data-kind="in"
              data-key="0"
            >
              <div class="tx-flow__top">
                <Amount sats={totalOut(props.tx)} unit={false} />
                {openButton("in", 0)}
              </div>
              <span class="tx-flow__who">Coinbase</span>
              <span class="tx-flow__meta">New coins and fees</span>
            </li>
          }
        >
          <For each={vin().slice(0, inShown())}>
            {(input, i) => (
              <li
                class="tx-flow__item"
                classList={{ "is-selected": isSel("in", i()) }}
                data-value={input.prevout?.value ?? 0}
                data-key={i()}
              >
                <div class="tx-flow__top">
                  <Show when={input.prevout} fallback={<span class="amount">–</span>}>
                    {(p) => <Amount sats={p().value} unit={false} />}
                  </Show>
                  {openButton("in", i())}
                </div>
                <span class="tx-flow__who">
                  <Show
                    when={input.prevout?.scriptpubkey_address}
                    fallback={input.prevout ? scriptLabel(input.prevout) : "Unknown"}
                  >
                    {(a) => (
                      <Hash value={a()} href={paths.address(a())} head={7} tail={5} copy={false} />
                    )}
                  </Show>
                </span>
                <span class="tx-flow__meta">
                  {input.prevout?.scriptpubkey_address
                    ? classify(input.prevout.scriptpubkey).label
                    : ""}
                </span>
              </li>
            )}
          </For>
          <Show when={vin().length > inShown()}>
            {more("in", vin().length - inShown(), hiddenIn(), () => setInPresses(inPresses() + 1))}
          </Show>
        </Show>
      </ul>
      <svg class="tx-flow__ribbons" ref={svgEl} viewBox={view()} aria-hidden="true">
        {/* Index keeps each path element across redraws, so a resize never restarts or cuts an animation. */}
        <Index each={ribbons()}>
          {(r) => (
            <path
              class={`tx-ribbon tx-ribbon--${r().kind}${
                props.selected !== null &&
                r().key === `${props.selected.side === "in" ? "i" : "o"}${props.selected.index}`
                  ? " is-selected"
                  : ""
              }`}
              data-side={r().key.startsWith("i") ? "in" : "out"}
              d={r().d}
              style={r().kind === "fee" ? { "--swatch": feeColor(rate()) } : undefined}
            />
          )}
        </Index>
        <rect
          class="tx-flow__trunk"
          x={trunk().x - 3}
          y={trunk().y - 4}
          width={6}
          height={trunk().h + 8}
          rx={3}
        />
      </svg>
      <ul class="tx-flow__col tx-flow__col--out" aria-label="Outputs" ref={outCol}>
        <For each={props.tx.vout.slice(0, outShown())}>
          {(out, i) => (
            <li
              class="tx-flow__item"
              classList={{ "is-selected": isSel("out", i()) }}
              data-value={out.value}
              data-key={i()}
            >
              <div class="tx-flow__top">
                <Amount sats={out.value} unit={false} />
                {openButton("out", i())}
              </div>
              <span class="tx-flow__who">
                <Show
                  when={out.scriptpubkey_address}
                  fallback={<span class="tx-flow__label">{scriptLabel(out)}</span>}
                >
                  {(a) => (
                    <Hash value={a()} href={paths.address(a())} head={7} tail={5} copy={false} />
                  )}
                </Show>
              </span>
              <span class="tx-flow__meta">
                <Show when={out.scriptpubkey_address}>
                  <span>{classify(out.scriptpubkey).label}</span>
                </Show>
                <Spent out={out} spend={props.outspends()?.[i()]} txid={props.tx.txid} />
              </span>
            </li>
          )}
        </For>
        <Show when={props.tx.vout.length > outShown()}>
          {more("out", props.tx.vout.length - outShown(), hiddenOut(), () =>
            setOutPresses(outPresses() + 1),
          )}
        </Show>
        <Show when={fee() > 0}>
          <li
            class="tx-flow__item tx-flow__item--fee"
            style={{ "--swatch": feeColor(rate()) }}
            data-value={fee()}
            data-kind="fee"
            data-key="fee"
          >
            <div class="tx-flow__top">
              <Amount sats={fee()} unit={false} />
            </div>
            <span class="tx-flow__who">Fee</span>
            <span class="tx-flow__meta">
              <FeeRate rate={rate()} />
            </span>
          </li>
        </Show>
      </ul>
    </div>
  );
}

/** Whether an output has been spent, and by which transaction. Nothing until the answer arrives. */
function Spent(props: { out: Vout; spend: Outspend | undefined; txid: string }) {
  return (
    <Show
      when={classify(props.out.scriptpubkey).kind !== "op_return"}
      fallback={<span>Unspendable</span>}
    >
      <Show when={props.spend}>
        {(s) => (
          <Show when={s().spent && s().txid} fallback={<span>Unspent</span>}>
            {(by) => (
              <Link href={paths.tx(by())} class="tx-flow__spent">
                {s().status?.confirmed === false ? "Being spent" : "Spent"}
                <span class="visually-hidden"> in transaction {by()}</span>
              </Link>
            )}
          </Show>
        )}
      </Show>
    </Show>
  );
}
