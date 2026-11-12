// One Lightning channel: its ends and what each charges, where it was funded, and how it closed
// and how sure that is (docs/LIGHTNING.md "How a close is classified").
import { useParams } from "@solidjs/router";
import { Match, Show, Switch } from "solid-js";
import { Amount, Sats } from "../../components/Amount";
import { Hash } from "../../components/Hash";
import { Link } from "../../components/Link";
import { More } from "../../components/More";
import { SectionHeader } from "../../components/SectionHeader";
import { Empty, ErrorState, Loading } from "../../components/States";
import { TimeAgo, TimeUtc } from "../../components/Time";
import { createRequest, isNotFound } from "../../lib/api";
import { groupDigits } from "../../lib/format";
import { type Network, paths } from "../../lib/network";
import { useNetwork, usePageTitle } from "../../lib/page";
import {
  type BlockRef,
  type ChannelDetail,
  CLOSE_WORDS,
  type End,
  isChannelDetail,
  isChannelId,
  msat,
  ppm,
  STATE_WHY,
  STATE_WORDS,
} from "./lightning";
import { ask, BlockLink, Gate, NodeName, Stat } from "./parts";

export default function ChannelPage() {
  const params = useParams<{ id: string }>();
  const network = useNetwork();
  const id = () => params.id ?? "";
  usePageTitle(() => `Lightning channel ${id()}`);
  return (
    <div class="wrap ln">
      <p class="ln-back">
        <Link href={paths.lightning()}>Lightning</Link>
      </p>
      <Show
        when={isChannelId(id())}
        fallback={
          <NotFound title="Not a channel id">
            A channel is named by its funding position, block, transaction and output, as in
            811984x2037x0, or by the same as one decimal number.
          </NotFound>
        }
      >
        <Show when={{ network: network(), id: id() }} keyed>
          {(k) => (
            <Gate>
              <Channel network={k.network} id={k.id} />
            </Gate>
          )}
        </Show>
      </Show>
    </div>
  );
}

function NotFound(props: { title: string; children: string }) {
  return (
    <>
      <h1>Lightning channel</h1>
      <Empty title={props.title}>{props.children}</Empty>
    </>
  );
}

function Channel(props: { network: Network; id: string }) {
  const [channel, { refetch }] = createRequest(
    () => props.id,
    (id, signal) => ask(props.network, `/channels/${id}`, isChannelDetail, signal),
  );
  return (
    <Switch>
      <Match when={channel.latest}>{(c) => <Body channel={c()} />}</Match>
      <Match when={channel.state === "errored" && isNotFound(channel.error)}>
        <NotFound title="This server knows no such channel">
          It lists only public channels whose funding output it found on the chain, from when its
          Lightning peer started. A private channel, one that closed before then, or one announced
          but never funded is not here.
        </NotFound>
      </Match>
      <Match when={channel.state === "errored"}>
        <ErrorState title="Could not load this channel" error={channel.error} retry={refetch} />
      </Match>
      <Match when={true}>
        <Loading label="Loading the channel" />
      </Match>
    </Switch>
  );
}

function When(props: { block: BlockRef }) {
  return (
    <>
      Block <BlockLink block={props.block} />
      <span class="ln-fact__sub">
        <TimeUtc time={props.block.time} />, <TimeAgo time={props.block.time} />
      </span>
    </>
  );
}

function Body(props: { channel: ChannelDetail }) {
  const c = () => props.channel;
  return (
    <>
      <header class="ln-identity">
        <h1>
          Channel <span class="num">{c().id}</span>{" "}
          <span class={["ln-badge", { "ln-badge--closed": c().state === "closed" }]}>
            {STATE_WORDS[c().state] ?? c().state}
          </span>
        </h1>
        <p>
          {STATE_WHY[c().state] ?? ""} Its short channel id as one number is{" "}
          <span class="num">{c().scid}</span>.
        </p>
      </header>

      <section aria-labelledby="ln-channel-facts-title">
        <SectionHeader title="Funding" id="ln-channel-facts-title" />
        <dl class="ln-stats">
          <Stat label="Capacity">
            <Amount sats={c().capacity} />
          </Stat>
        </dl>
        <dl class="ln-facts">
          <div class="ln-fact">
            <dt>Funding transaction</dt>
            <dd>
              <Show
                when={c().funding.txid}
                fallback={
                  <>
                    Not looked up yet
                    <span class="ln-fact__sub">
                      This server asks the node for each channel's funding transaction in the
                      background.
                    </span>
                  </>
                }
              >
                {(txid) => (
                  <>
                    <Hash value={txid()} href={paths.tx(txid())} copyLabel="Copy transaction ID" />
                    <span class="ln-fact__sub">Output {groupDigits(c().funding.vout)}</span>
                  </>
                )}
              </Show>
            </dd>
          </div>
          <div class="ln-fact">
            <dt>Funded in</dt>
            <dd>
              <Show when={c().funding.block} fallback="Not recorded yet">
                {(b) => <When block={b()} />}
              </Show>
            </dd>
          </div>
          <div class="ln-fact">
            <dt>Checked against the chain</dt>
            <dd>
              Yes
              <span class="ln-fact__sub">
                Its funding output pays the two keys the channel was announced with; the capacity is
                that output's value.
              </span>
            </dd>
          </div>
        </dl>
      </section>

      <Show when={c().closed}>
        {(x) => {
          const words = () => CLOSE_WORDS[x().kind];
          return (
            <section aria-labelledby="ln-channel-close-title">
              <SectionHeader title="Close" id="ln-channel-close-title" />
              <div class="ln-close">
                <p class="ln-close__title">
                  {words().name} <span class="ln-sure">· {words().sure}</span>
                </p>
                <p>{words().why}</p>
                <Show when={x().kind === "force" && c().penalty === null}>
                  <More label="About penalties">
                    <p>
                      Had the other end cheated, its penalty would show here once swept, if that
                      happens within 2 016 blocks while this server watches.
                    </p>
                  </More>
                </Show>
              </div>
              <dl class="ln-facts">
                <div class="ln-fact">
                  <dt>Closing transaction</dt>
                  <dd>
                    <Hash
                      value={x().txid}
                      href={paths.tx(x().txid)}
                      copyLabel="Copy transaction ID"
                    />
                  </dd>
                </div>
                <div class="ln-fact">
                  <dt>Closed in</dt>
                  <dd>
                    <When block={x().block} />
                  </dd>
                </div>
                <div class="ln-fact">
                  <dt>Closing fee</dt>
                  <dd>
                    <Show when={x().fee !== null} fallback="Not known">
                      <Sats sats={x().fee ?? 0} />
                    </Show>
                  </dd>
                </div>
                <Show when={c().penalty}>
                  {(p) => (
                    <div class="ln-fact">
                      <dt>Penalty sweep</dt>
                      <dd>
                        <Hash
                          value={p().txid}
                          href={paths.tx(p().txid)}
                          copyLabel="Copy transaction ID"
                        />
                        <span class="ln-fact__sub">
                          Block <BlockLink block={p().block} />
                        </span>
                      </dd>
                    </div>
                  )}
                </Show>
              </dl>
            </section>
          );
        }}
      </Show>

      <section aria-labelledby="ln-channel-ends-title">
        <SectionHeader title="Its two ends" id="ln-channel-ends-title">
          <Show when={c().last_update}>
            {(t) => (
              <>
                Last update <TimeAgo time={t()} />
              </>
            )}
          </Show>
        </SectionHeader>
        <div class="ln-sides">
          <Side end={c().node_one} other={c().node_two} closed={c().closed !== null} />
          <Side end={c().node_two} other={c().node_one} closed={c().closed !== null} />
        </div>
        <More label="About these">
          <p>
            Each end sets its own policy: what it charges to forward a payment across the channel to
            the other end.
          </p>
        </More>
      </section>
    </>
  );
}

function Side(props: { end: End; other: End; closed: boolean }) {
  return (
    <div class="ln-side">
      <h3>
        <NodeName node={props.end} />
      </h3>
      <Show
        when={props.end.policy}
        fallback={
          <p class="ln-aside">
            {props.closed
              ? "A closed channel has no policy."
              : "This end has sent no policy that this server holds."}
          </p>
        }
      >
        {(p) => (
          <dl class="ln-facts">
            <div class="ln-fact">
              <dt>Fee rate</dt>
              <dd class="num">{ppm(p().fee_rate)}</dd>
            </div>
            <div class="ln-fact">
              <dt>Base fee</dt>
              <dd class="num">{msat(p().base_fee_msat)}</dd>
            </div>
            <div class="ln-fact">
              <dt>Time lock delta</dt>
              <dd class="num">{groupDigits(p().cltv_delta)} blocks</dd>
            </div>
            <div class="ln-fact">
              <dt>Payments forwarded</dt>
              <dd class="num">
                {msat(p().min_htlc_msat)} to {msat(p().max_htlc_msat)}
              </dd>
            </div>
            <div class="ln-fact">
              <dt>Forwarding</dt>
              <dd>
                {p().enabled ? "Enabled" : "Disabled"}
                <span class="ln-fact__sub">
                  Updated <TimeAgo time={p().last_update} />
                </span>
              </dd>
            </div>
          </dl>
        )}
      </Show>
    </div>
  );
}
