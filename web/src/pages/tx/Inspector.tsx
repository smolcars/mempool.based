import { createMemo, createSignal, For, Match, Show, Switch } from "solid-js";
import type { JSX } from "@solidjs/web";
import { Amount } from "../../components/Amount";
import { Hash } from "../../components/Hash";
import type { Outspend, Transaction, Vin, Vout } from "../../lib/esplora";
import { groupDigits } from "../../lib/format";
import { paths } from "../../lib/network";
import { Code } from "./Code";
import type { Selection } from "./Flow";
import {
  classify,
  disassemble,
  hexToBytes,
  isText,
  lastPush,
  opReturnData,
  printable,
} from "./script";
import {
  coinbaseHeight,
  formatSequence,
  MORE_STEP,
  SHOW_ALL_MAX,
  sequenceMeaning,
  whyUnspendable,
} from "./tx";

/** Witness items listed at first; each "Show more" adds MORE_STEP. */
const WITNESS_SHOWN = 24;

const asm = (hex: string) => disassemble(hex, SHOW_ALL_MAX);

function Fact(props: { label: string; children: JSX.Element }) {
  return (
    <div class="tx-fact">
      <dt>{props.label}</dt>
      <dd>{props.children}</dd>
    </div>
  );
}

/** A script as hex and as assembly. */
function Script(props: { label: string; hex: string }) {
  const a = createMemo(() => asm(props.hex));
  return (
    <>
      <Code label={`${props.label}, hex`} value={props.hex} bytes={props.hex.length / 2} />
      <Show when={props.hex !== ""}>
        <Code
          label={`${props.label}, assembly`}
          value={a().text}
          cut={a().truncated}
          copy={false}
        />
      </Show>
    </>
  );
}

/** Data decoded as text when it reads as text, with invisible bytes made visible. */
function DataText(props: { label: string; bytes: Uint8Array }) {
  const p = createMemo(() => printable(props.bytes, SHOW_ALL_MAX));
  return <Code label={props.label} value={p().text} copy={false} cut={p().chars >= SHOW_ALL_MAX} />;
}

/** Everything about one input or output: scripts, witness, sequence, what it spends or what spent it. */
export function Inspector(props: {
  tx: Transaction;
  selected: Selection;
  outspends: () => Outspend[] | null | undefined;
  onClose: () => void;
  ref: (el: HTMLHeadingElement) => void;
}) {
  const title = () =>
    `${props.selected.side === "in" ? "Input" : "Output"} ${props.selected.index}`;
  return (
    <section class="tx-inspector" id="tx-inspector" aria-labelledby="tx-inspector-title">
      <div class="tx-inspector__head">
        <h3 id="tx-inspector-title" tabindex="-1" ref={props.ref}>
          {title()}
        </h3>
        <button class="btn" type="button" onClick={() => props.onClose()}>
          Close
        </button>
      </div>
      <Switch>
        <Match when={props.selected.side === "in" && props.tx.vin?.[props.selected.index]}>
          {(vin) => <InputDetails vin={vin()} version={props.tx.version} />}
        </Match>
        <Match when={props.selected.side === "out" && props.tx.vout[props.selected.index]}>
          {(vout) => (
            <OutputDetails
              vout={vout()}
              spend={props.outspends()?.[props.selected.index]}
              failed={props.outspends() === null}
              unspendable={whyUnspendable(props.tx, vout())}
            />
          )}
        </Match>
      </Switch>
    </section>
  );
}

function InputDetails(props: { vin: Vin; version: number }) {
  const prevKind = () =>
    props.vin.prevout ? classify(props.vin.prevout.scriptpubkey).kind : undefined;
  const witness = () => props.vin.witness ?? [];
  // The script a P2SH, P2WSH or taproot script-path spend reveals, named the way its type names it.
  const inner = createMemo((): { label: string; hex: string } | null => {
    const w = witness();
    const kind = prevKind();
    if (kind === "p2wsh" && w.length > 0) return { label: "Witness script", hex: w.at(-1) ?? "" };
    if (kind === "p2tr") {
      const stack = w.length > 1 && w.at(-1)?.startsWith("50") ? w.slice(0, -1) : w;
      return stack.length >= 2 ? { label: "Tapscript", hex: stack.at(-2) ?? "" } : null;
    }
    if (kind === "p2sh") {
      const redeem = lastPush(props.vin.scriptsig ?? "");
      if (!redeem) return null;
      const c = classify(redeem).kind;
      if (c === "p2wsh" && w.length > 0) return { label: "Witness script", hex: w.at(-1) ?? "" };
      return { label: "Redeem script", hex: redeem };
    }
    return null;
  });
  const [witnessPresses, setWitnessPresses] = createSignal(0);
  const witnessShown = () =>
    Math.min(witness().length, WITNESS_SHOWN + witnessPresses() * MORE_STEP);
  return (
    <>
      <dl class="tx-facts">
        <Show
          when={!props.vin.is_coinbase}
          fallback={
            <>
              <Fact label="Spends">New coins: this is a coinbase</Fact>
              <Show when={coinbaseHeight(props.vin.scriptsig)}>
                {(h) => <Fact label="Block height in script">{groupDigits(h())}</Fact>}
              </Show>
            </>
          }
        >
          <Fact label="Spends">
            <span class="tx-outpoint">
              <Hash
                value={props.vin.txid}
                href={paths.tx(props.vin.txid)}
                copyLabel="Copy transaction ID"
              />
              <span class="num">:{props.vin.vout}</span>
            </span>
          </Fact>
          <Show when={props.vin.prevout}>
            {(p) => (
              <>
                <Fact label="Amount">
                  <Amount sats={p().value} />
                </Fact>
                <Fact label="Address">
                  <Show when={p().scriptpubkey_address} fallback="None">
                    {(a) => (
                      <Hash
                        value={a()}
                        href={paths.address(a())}
                        head={14}
                        tail={10}
                        copyLabel="Copy address"
                      />
                    )}
                  </Show>
                </Fact>
                <Fact label="Script type">{classify(p().scriptpubkey).label}</Fact>
              </>
            )}
          </Show>
        </Show>
        <Show when={props.vin.sequence !== undefined}>
          <Fact label="Sequence">
            <span class="num">{formatSequence(props.vin.sequence ?? 0)}</span>
            <span class="tx-fact__sub">
              {sequenceMeaning(props.vin.sequence ?? 0, props.version)}
            </span>
          </Fact>
        </Show>
      </dl>
      <div class="tx-codes">
        <Show when={props.vin.is_coinbase}>
          <DataText label="Coinbase data as text" bytes={hexToBytes(props.vin.scriptsig ?? "")} />
        </Show>
        <Show when={(props.vin.scriptsig ?? "") !== ""}>
          <Script
            label={props.vin.is_coinbase ? "Coinbase script" : "Unlocking script (scriptSig)"}
            hex={props.vin.scriptsig ?? ""}
          />
        </Show>
        <Show when={witness().length > 0}>
          <div class="tx-witness">
            <p class="tx-witness__title">
              Witness, {groupDigits(witness().length)} item{witness().length === 1 ? "" : "s"}
            </p>
            <For each={witness().slice(0, witnessShown())}>
              {(item, i) => <Code label={`Item ${i()}`} value={item} bytes={item.length / 2} />}
            </For>
            <Show when={witness().length > witnessShown()}>
              <button class="btn" type="button" onClick={() => setWitnessPresses((n) => n + 1)}>
                Show {groupDigits(Math.min(witness().length - witnessShown(), MORE_STEP))} more of{" "}
                {groupDigits(witness().length)} items
              </button>
            </Show>
          </div>
        </Show>
        <Show when={inner()}>{(s) => <Script label={s().label} hex={s().hex} />}</Show>
        <Show when={props.vin.prevout}>
          {(p) => <Script label="Spent output's script" hex={p().scriptpubkey} />}
        </Show>
      </div>
    </>
  );
}

function OutputDetails(props: {
  vout: Vout;
  spend: Outspend | undefined;
  failed: boolean;
  unspendable: string | null;
}) {
  const kind = () => classify(props.vout.scriptpubkey);
  const data = createMemo(() =>
    kind().kind === "op_return" ? opReturnData(props.vout.scriptpubkey) : null,
  );
  return (
    <>
      <dl class="tx-facts">
        <Fact label="Amount">
          <Amount sats={props.vout.value} />
        </Fact>
        <Fact label="Address">
          <Show when={props.vout.scriptpubkey_address} fallback="None">
            {(a) => (
              <Hash
                value={a()}
                href={paths.address(a())}
                head={14}
                tail={10}
                copyLabel="Copy address"
              />
            )}
          </Show>
        </Fact>
        <Fact label="Script type">{kind().label}</Fact>
        <Fact label="Spent">
          <Switch fallback="Checking…">
            <Match when={props.unspendable}>{(why) => `Unspendable: ${why()}`}</Match>
            <Match when={props.failed}>
              Could not check right now. Reload the page to try again.
            </Match>
            <Match when={props.spend?.spent === false}>No</Match>
            <Match when={props.spend?.spent && props.spend.txid}>
              {(by) => (
                <span class="tx-outpoint">
                  <Show when={props.spend?.status?.confirmed === false}>
                    Being spent, unconfirmed, by{" "}
                  </Show>
                  <Hash value={by()} href={paths.tx(by())} copyLabel="Copy transaction ID" />
                  <Show when={props.spend?.vin !== undefined}>
                    <span class="num">, input {props.spend?.vin}</span>
                  </Show>
                </span>
              )}
            </Match>
          </Switch>
        </Fact>
      </dl>
      <div class="tx-codes">
        <Show when={data()}>
          {(d) => (
            <Show
              when={isText(d())}
              fallback={
                <Code
                  label="Data"
                  value={[...d()].map((b) => b.toString(16).padStart(2, "0")).join("")}
                  bytes={d().length}
                />
              }
            >
              <DataText label="Data as text" bytes={d()} />
            </Show>
          )}
        </Show>
        <Script label="Locking script (scriptPubKey)" hex={props.vout.scriptpubkey} />
      </div>
    </>
  );
}
