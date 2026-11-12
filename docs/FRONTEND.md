# Frontend

How the Solid app in `web/` is put together, and the rules that let several people build pages at once without colliding. The visual rules are in [`DESIGN.md`](DESIGN.md); this file is about code.

## Layout

```text
web/
  .oxlintrc.json        Lint rules, and the reason for every rule that is off (see "Checks")
  .oxfmtrc.json         Formatter settings
  tsconfig.json         Shared compiler options; tsconfig.app.json (browser) and tsconfig.node.json extend it
  index.html            The shell document: preloads the font, runs public/boot.js, loads src/index.tsx
  public/boot.js        Runs before first paint: applies a stored theme and the network (see "Theme")
  scripts/check-size.js First-load budget check (`bun run size`)
  scripts/e2e.sh        Browser tests in one command (`bun run e2e`)
  e2e/                  Browser tests (Playwright) and their fixtures
  src/
    index.tsx           Waits briefly for the font, then mounts the router
    routes.ts           The route table: every page, on every network
    styles/global.css   Layer order, reset, base and layout. Imports nothing; tokens come from design/
    lib/                Pure logic and hooks, no markup
      format.ts         Amounts, fee rates and bands, sizes, hashes, times
      network.ts        Networks, prefixes, `paths` for every page
      address.ts        An address's type and networks from its form (the address page and search)
      api.ts            HTTP client and `createApi` resource
      live.ts           The WebSocket client: `useTopic`, `useLiveStatus` (see "Live data")
      blocks-topic.ts   The `blocks` topic: its messages and how to keep a block list current
      projected-topic.ts The `projected` topic: every projected block's totals and the tip they sit on
      projected-txs.ts  The `projected-txs:N` topics: binary frames, and a block's transactions kept from them
      mempool-topics.ts The `mempool` and `transactions` topics: their messages, rates in sat/vB
      summary.ts        Block summaries: their type, a check, a page-wide cache and the batch fetch
      fakes.ts          Test stand-ins: a fake WebSocket and made-up blocks
      esplora.ts        Types for the Esplora replies pages use
      page.ts           `useNetwork`, `usePageTitle`
      clock.ts          Shared ticking clock for relative times
      config.ts         Reads `/app/config`
      chunks.ts         A page's code and styles that failed to download, and asking for them again
      theme.ts          Theme toggle state
    components/         Shared components, one CSS file each
      txgrid/           The transaction grid: decoder, layout, animation, WebGL2 renderer (see "Transaction grid")
    shell/              Header, network switch, theme toggle, band, footer, the frame around pages
    search/             What a query is (query.ts) and where it leads (search.ts); loaded on first use
    pages/<page>/       One directory per page: its routes, component, CSS and tests
```

## Routing

`@solidjs/router`. Mainnet pages are at the root and the other networks under `/signet`, `/testnet4` and `/regtest`, matching the server ([`ARCHITECTURE.md`](ARCHITECTURE.md)). `src/routes.ts` mounts every page's routes under all four prefixes, so a page is written once with network-free paths (`/tx/:txid`) and works on every network. The server sends the app shell for any page path, so deep links and reloads work; a disabled network's paths are a 404 from the server.

The network comes from the path. `useNetwork()` returns it; never keep it anywhere else.

| Page | Path | Directory | Built in |
|---|---|---|---|
| Home | `/` | `pages/home` | W2 |
| Block | `/block/:id` (hash or height) | `pages/block` | W3 |
| Blocks | `/blocks`, `/blocks/:from` (paging back from a height) | `pages/blocks` | W3 |
| Projected block | `/block/next`, `/block/next/:n` (2 to 8 places from now; 8 is the rest of the mempool) | `pages/projected` | W8 |
| Transaction | `/tx/:txid` | `pages/tx` | W4 |
| Address | `/address/:address` | `pages/address` | W5 |
| Mining | `/mining` | `pages/mining` | W9 |
| Lightning | `/lightning` | `pages/lightning` | W11 |
| Not found | anything else | `pages/not-found` | shell |

Every page is loaded lazily. The pages above are stubs that say which task builds them (`pages/Stub.tsx`); delete the stub's use when you build the page, and the last page built deletes the file.

### Adding a page or a route

1. Create `pages/<page>/` with `<Name>Page.tsx` (default export) and `routes.ts`:

   ```ts
   export const routes: RouteDefinition[] = [
     { path: "/blocks", component: lazy(() => import("./BlocksPage")) },
   ];
   ```

2. Spread `routes` into the list in `src/routes.ts`, before `notFound`. That one line is the only edit outside your directory.

Sub-pages (`/mining/pool/:slug`) go in the same `routes.ts`; you never touch `src/routes.ts` again.

### Links

Internal links stay on the current network. Use `<Link href={paths.tx(txid)}>` from `components/Link.tsx`; it adds the prefix and the router handles the click. Build every path with `paths` in `lib/network.ts`, which encodes chain data into a single path segment so it can never change the route or the scheme ([`SECURITY.md`](SECURITY.md) rule 14). Add a function to `paths` for a new page rather than writing the path by hand. To navigate from code: `navigate(onNetwork(network(), paths.block(h)))`.

### Page title

Call `usePageTitle(() => "Block 975 411")`. Test networks get their name in front, and every title ends in `| mempool.based`.

## Data

### The public API

`lib/api.ts` is the only way pages talk to `/api`. Paths are relative to the network's API, so the same code works on every network.

```tsx
const params = useParams<{ id: string }>();
const [block] = createApi<Block>(() => `/block/${encodeURIComponent(params.id)}`);

<Show when={block()} fallback={<Loading label="Loading block" />}>
  {(b) => <h1>Block {groupDigits(b().height)}</h1>}
</Show>
```

- `createApi<T>(path, { as: "text" })` is a Solid resource. It refetches when the path or the network changes, aborts the request it replaces and the one in flight when the page unmounts, answers at once from the cache, and stays loading while a busy refusal waits to be retried. Return a falsy path to wait for something else first.
- `fetchApi<T>(network, path, { as, signal })` is the same call as a promise, for event handlers and loops. `fetchApp<T>` is the same for the app's own API under the network's `/app`.
- **Chain-tip stamps.** `createApi<T>(path, { stamped: true })` and `fetchStamped<T>(network, path)` give `{ value, tip }`, where `tip` is the `X-Bitcoin-Chain-Tip` and its height that the reply is as of (undefined for an unstamped or cached reply). Take every figure that depends on the tip from one stamped reply, or compare the stamps of two: the block page counts confirmations from its status reply's stamp, and the home page's difficulty panel asks again while its answer's stamp trails the strip's tip.
- **Busy servers.** Every call above retries a 503 or 429 by itself, no sooner than its `Retry-After` (kept on `ApiError.retryAfter`), backing off from 1 to 30 s with a little jitter, six times, then rejects with the last error. They are all GETs, safe to repeat. Two 503s are not retried, because a retry cannot change them: an address too large to total and an address index that is off ([`API.md`](API.md#error-bodies-are-ours)). Show a loading state, not an error, while a call is pending. `withRetry(call, signal)` is the loop itself, for a call that does not go through `api.ts`; only wrap a call that is safe to repeat. A refusal it already gave up on is never retried by a loop around it. The browser still logs each refused request to the console; browser tests that may meet one allow `/status of (429|503)/`.
- `as: "text"` reads plain-text replies (`/blocks/tip/height`, `/block-height/:h`, `/tx/:txid/hex`), and `as: "bytes"` binary ones as an `ArrayBuffer` (`fetchApp(n, "/block/<hash>/txs", { as: "bytes" })`). The default is JSON.
- Failures are `ApiError` with `status` (0 when the server cannot be reached, with a message that says so when the browser is offline) and a message fit to show. `isNotFound(e)` tests for 404. An abort is passed through as the abort error, not as an `ApiError`.
- Replies the server marks as never changing (`immutable`, or `max-age` of a day or more) are kept in memory, at most 256, for the life of the page. Everything else is fetched every time and left to the browser's HTTP cache.
- Reading a resource inside the shell's `<Suspense>` holds the whole page until it loads. To show the page frame first and fill sections as they arrive, give each section its own `<Suspense>` or test `resource.loading`.
- An error thrown while rendering reaches the shell's error boundary, which shows "This page could not be shown". Catch expected failures (not found, node unavailable) in the page with `<ErrorState>` instead.

### When a page's files fail to load

Every page, and the transaction grid, is a chunk of code with its own stylesheet, downloaded on first use. A download can fail: a dropped connection, a busy proxy, or a deploy that replaced the files while a tab was open. Vite never asks for a failed stylesheet again by itself, so `lib/chunks.ts` notes every `<link rel="stylesheet">` that fails (`watchStyles`, started in `index.tsx`) and `retryStyles()` asks for each again through a fresh `<link>`. A failed script is fetched again when its `import()` runs again, which the router's `lazy()` does on the next render.

The shell's error boundary handles both. On a chunk error it shows "Loading this page" and tries again after 1, 2 and 4 s: before each attempt it asks `/app/config` (never from a cache) whether the server's version changed since the tab loaded, then asks again for the failed stylesheets, and renders the page again only once they are in, so a page never appears unstyled. When the attempts run out it says that part of the page could not be downloaded, with "Try again", which asks for everything again. When the version changed, the files this tab needs were replaced by a deploy: it says the site has been updated and offers "Reload" rather than retrying files that are gone. A navigation starts the count afresh.

`lib/esplora.ts` has hand-written types for the replies pages use, from rbitcoin v0.9.2's `docs/esplora-openapi.yaml`. Add a type when you use a new route; keep the field names exactly as the API sends them. Amounts are integer sats and times are unix seconds throughout.

During development, `bun run dev` proxies `/api`, `/app` and `/healthz` to the Rust server at `LISTEN_ADDR`.

### The app's own API

`GET /app/config` answers `{"networks": ["mainnet", "signet"], "version": "0.1.0"}`: the enabled networks in server order, and the server version. It is server-wide, so it is served at the root only, whichever networks are enabled (`crates/server/src/app_config.rs`). The shell reads it once for the network switch and the footer. If the root's network (mainnet) is disabled, the shell moves to the same page on the first enabled network.

#### Block summaries

What the public API cannot give cheaply: a mined block's fee facts and every transaction's fee and size. Per network, under its `/app` (`crates/server/src/summary.rs`).

| Route | Answer |
|---|---|
| `GET /app/block/:hash/summary` | The block's facts, JSON (below) |
| `GET /app/block/:hash/txs` | Every transaction as a fixed-width binary record (below) |
| `GET /app/blocks/summary?hashes=<hash>,<hash>,…` | 1 to 10 blocks' facts, a JSON array in the order asked. Repeats are allowed |

```json
{
  "id": "00000000000000000000e13ad5e424f6ba7208b0c1a9a687e9ccc4d62b3b094c", "height": 975511,
  "tx_count": 4198, "fees": 1475029, "vsize": 999391, "weight": 3991349,
  "fee_rate": {"min": 0.10071942446043165, "p10": 0.12, "p25": 0.12698412698412698,
    "median": 1.0850888512305468, "p75": 2.007012551450785, "p90": 3.150749063670412, "max": 62.10091743119266},
  "fee_bands": [367013, 9447, 153, 5069, 233233, 20986, 238267, 102460, 17310, 5453]
}
```

- `tx_count` counts every transaction. Everything else leaves the coinbase out, as `getblockstats` does: `fees` in sats, `vsize` and `weight` summed over the paying transactions.
- `fee_rate` is in sat/vB, unrounded, `null` for a block holding only its coinbase. Percentiles are weighted by virtual size: `p25` is the rate of the transaction where the cheapest quarter of the block's vbytes ends. Print them with `formatFeeRate`.
- `fee_bands` is the virtual size paying in each of the ten design bands, cheapest first: the strata. They sum to `vsize`. Each transaction is banded on its rate as printed, as `feeBand` does, so a stratum's color agrees with the numbers beside it. How full the block is comes from the block's own `weight` (Esplora) over 4,000,000.

`/txs` is `application/octet-stream`, little-endian: an 8-byte header, then one 20-byte record per transaction in block order, the coinbase first.

| Offset | Size | Field |
|---|---|---|
| 0 | `u32` | Format version, `1` |
| 4 | `u32` | Record count, `tx_count` |
| 8 + 20·i | `u32` | Virtual size of transaction `i` |
| 12 + 20·i | `u64` | Fee in sats; 0 for the coinbase |
| 20 + 20·i | `u64` | Total output value in sats |

Read it with a `DataView`; a `u64` here is below 2^53, so `Number(view.getBigUint64(o, true))` is exact. A record has no txid: when a square is hovered or clicked, ask `/api/block/:hash/txid/:index`, which is cached for good. Mainnet block 975 511, 4,198 transactions, is 83,968 bytes: 16.6 KB gzipped and 12.7 KB with brotli, if the proxy in front compresses (the server itself does not). The facts are about 400 bytes. Check the version before reading; a different one means a newer server and a page that needs a reload.

**In the frontend.** `lib/summary.ts` has the type, `isSummary`, and `fetchSummaries(network, hashes, signal)`, which asks for up to ten at a time with `withRetry` and keeps every summary it sees, fetched or pushed, for the life of the page (`knownSummary`). The home page asks for its ten tiles in one batch on first paint and then takes each new block's from the WebSocket, asking over HTTP only if the push has not come within 4 s. The blocks list asks per row, two at a time, so an old page fills in row by row. Draw the bands with `Strata`, and the records with the transaction grid below.

**Caching.** A summary is fixed by its hash, so success is `Cache-Control: public, max-age=31536000, immutable`, and `api.ts`-style in-memory caching is right. Errors are `no-store`.

**Cost and limits.** A block in the block table with its fee rates ([`STORE.md`](STORE.md#blocks)) costs nothing: once backfilled, the facts of the last year's blocks come from there. `/txs` always needs the node. An uncached block costs the node a heavy call (about 0.7 s and 10 MB on mainnet), so:

- Concurrent requests for one block share one computation, and the facts and the records come from the same one.
- The newest ten blocks are computed as they arrive, so the home page's tiles are always cached. A new block's facts are also pushed on the WebSocket as a `summary` message; use that rather than fetching.
- Each network computes one block at a time, and holds at most six started by clients. Past that the answer is `503` with `Retry-After: 1`. Retry: the work already admitted goes on, so a retry finds more of it cached. A cold batch of ten needs a retry or two.
- Each client may start ten computations at once, then one every 2 s; past that `429` with `Retry-After: 2`. Cached blocks never count. A page walking back through history should fetch tiles lazily, as they scroll into view.
- `404` is an unknown block, `400` a malformed hash or query, `502` and `504` the node failing. All are worth a "Try again".

### Live data

`lib/live.ts` holds one WebSocket per tab and network to `/app/ws` ([`WS.md`](WS.md)). It opens while any component is subscribed to a topic and closes when none is. It checks the greeting's protocol version and network, subscribes after it, reconnects with exponential backoff and jitter (0.5 to 30 s), resubscribes, pings every 30 s to find a dead socket, sends subscription changes at most every 250 ms (unsubscribes before it subscribes, so switching between capped topics never holds one too many), and ignores message types it does not know. Binary frames are read as `ArrayBuffer`s and offered to every subscribed topic that has a `binary` reader; the others never see them. A hidden tab keeps its socket; a socket that drops while hidden, or while the browser is offline, waits until the tab is visible and online again. After close codes 1002–4002 or a greeting from another protocol version it stops for good: the page is out of date.

A topic is a name and a parser, and a binary reader if it sends binary frames. Write one per topic in `lib/`, as `blocks-topic.ts` does; the client never changes for a new topic:

```ts
export const mempoolTopic: Topic<MempoolMessage> = {
  name: "mempool",
  // Return undefined for a type or shape this page does not know: it is ignored.
  parse: (m) => (m.type === "delta" && isDelta(m.delta) ? { type: "delta", delta: m.delta } : undefined),
};
```

A component uses it for as long as it is mounted, on the page's network:

```ts
useTopic(blocksTopic, {
  message: (m) => { /* every message, in order; the first is the topic's current state */ },
  resync: () => { /* the socket reconnected and messages may be lost: refetch over HTTP */ },
});
const status = useLiveStatus(); // "connecting" | "live" | "reconnecting" | "offline"
```

- **Load over HTTP, then keep up over the socket.** The socket only reports changes. Fetch the state with `fetchApi`, apply messages on top, and treat a gap, a `resync` message that disagrees and the `resync` handler the same way: fetch again. Messages that arrive while that fetch is in flight are held and replayed on its answer (`pages/home/feeds.ts`, `createBlockFeed`).
- **The mempool topics** exist now ([`WS.md`](WS.md#mempool-topics)): `mempool` for the mempool panel and bar, `projected` for the projected tiles, `transactions` for the latest transactions, and `projected-txs:N` for one projected block's grid. The first three are JSON and each message is the whole state, so a page keeps the last one; `projectedTopic` in `lib/projected-topic.ts` reads `projected`, with rates turned into sat/vB. `lib/mempool-topics.ts` reads `mempool` and `transactions` the same way. On a server with the mempool switched off each topic sends `{"type":"off"}` once. Rates on the wire are in sat per 1,000 vB.
- **A projected block's transactions** (`lib/projected-txs.ts`). `projectedTxsTopic(n)` reads block `n`'s frames with `decodeFrame`, which never throws: a frame of another kind is ignored, one in a newer format version comes back as `version` (ask for a reload), and one that is cut short, too long, has nonzero padding or a fee beyond 2^53 comes back as `malformed`. `BlockTxs` keeps the block as a `Map` from txid to a `GridTx`-shaped record (`{ id, vsize, fee, rate }`, rate in sat/vB), the same object until its record changes. `apply(frame)` answers `changed` (with the txids that came in and went out), `lost`, `ignored` or `version`. On `lost`, call the function `useTopic` returns: it unsubscribes and subscribes again in the next batch (`LiveConnection.restart`), and the snapshot that follows starts the block over. Until it comes the model keeps the last picture and ignores the old subscription's deltas still in flight, so one gap never becomes a loop. Call `expectSnapshot()` from the topic's `resync` handler: a reconnect subscribes afresh. A page holds at most two of these topics at once, and leaving the page unsubscribes. `fakes.ts` builds frames for tests (`snapshotFrame`, `deltaFrame`, `fakeTxid`).
- **The home page's mempool** (`pages/home/live-mempool.ts`) takes the panels and the strip from those topics and asks `/api` for nothing they send. It polls `/mempool` and `/mempool/recent` instead only when the server says `off`, while the socket is down, or when the topics have said nothing 5 s after the page opened (a server still making its first mempool cycle). `stale: true` keeps the last state on screen and turns the live indicator to "Node not answering".
- **Say when it is not live.** While `status()` is not `live`, poll what the socket would have sent, no faster than every few seconds, and tell the reader (the home page's indicator).
- In tests, stub the global `WebSocket` with `FakeSocket` from `lib/fakes.ts` and play the server: `FakeSocket.last().hello()`, `.receive({...})`, `.drop(1006)`.

## Shared components

Import from `components/`. All of them render chain data as text only.

| Component | Use |
|---|---|
| `Amount sats unit? signed?` | BTC with eight decimals, trailing zeros dimmed, sats in the title. `signed` adds `+` and the positive color to incoming amounts. `unit={false}` hides the unit from sight only, for a table that names it in its header: screen readers still read it |
| `Sats sats unit?` | Grouped sats. `unit` as for `Amount` |
| `FeeRate rate unit? approx?` | Fee rate with its band's swatch. `approx` adds `~`. `unit` as for `Amount` |
| `FeeKey` | The ten bands with their lower bounds. Put it next to every fee visual |
| `Strata layers settle? from?` | A block's fee bands as stacked layers, cheapest at the bottom, filling the box it is put in. Compute `layers` with `strataLayers(fee_bands, fill, min)`: `fill` is how full the block is, `min` the thinnest layer as a share of the box (about 2 px). `settle` drops the layers in, cheapest first; `from` grows each layer out of the same band's layer in another set (a projected block becoming the mined one), a band it lacks from nothing; reduced motion fades either. Decorative: give the figures, or `describeBands(fee_bands)`, as text beside it |
| `TxGrid txs capacity label txid highlight? onState?` | Transactions as packed squares in WebGL2, in `components/txgrid/`. Load it with `lazy()`; see "Transaction grid" |
| `Hash value href? head? tail? copy? copyLabel?` | Hash or address, shortened in the middle, linked when given a `paths` result, with copy |
| `HashFull value copyLabel?` | A page subject's hash in full, grouped by eight, with copy |
| `CopyButton value label?` | The copy button on its own |
| `TimeAgo time suffix?` | Relative time that keeps itself current, UTC in the title |
| `TimeUtc time` | Absolute UTC time |
| `DataTable caption columns rows empty? rowClass?` | Table with units in headers. Columns: `header`, `cell(row, i)`, `numeric`, `optional` (dropped on phones). `rowClass(row)` classes one row, such as a highlight for a new one |
| `SectionHeader title id? children?` | A section's `h2` over the 2 px rule; `children` sit at the right |
| `Loading label?` | Waiting. Fades in only after 300 ms, so fast loads show nothing |
| `Empty title children?` | Nothing to show, and what to do about it |
| `ErrorState title error? message? retry? action?` | What failed, the API's message (or `message`), and a button: "Try again", or `action` |
| `Pagination page pageCount? hasNext? onPage label` | Previous and next. Esplora's cursor-paged lists pass `hasNext` instead of `pageCount` |
| `SearchField` | The search: reads the query as it is typed, opens its page on Enter or says why there is none. The header owns the one instance |
| `Link href` | Internal link on the current network |

Buttons are `<button class="btn">` (or `btn btn--accent`) with `components/Button.css`; toggles add `aria-pressed`.

For a figure without a component, use `lib/format.ts` directly: `formatBtc`, `formatSats`, `formatFeeRate`, `feeBand` and `feeColor`, `formatSize`, `groupDigits`, `truncateMiddle`, `formatUtc`, `formatRelative`. They never throw and print `–` for a value that is missing or malformed. Fee bands always come from `feeBand`, which bands the rate as printed.

## Transaction grid

`components/txgrid/` draws transactions as packed squares in WebGL2: area by virtual size, color by fee band, the highest fee rate at the top left, resting on the floor of the frame like sediment ([`DESIGN.md`](DESIGN.md#the-transaction-grid)). The block page and the projected-block page use it.

| File | Holds |
|---|---|
| `TxGrid.tsx` | The component (default export, so `lazy()` can load it) |
| `decode.ts` | `decodeTxs(buffer)`: the `/txs` records as `GridTx[]`. Throws `TxsFormatError` (`kind: "version"` asks for a reload) |
| `layout.ts` | `GridTx`, `layout(txs, aspect, capacity)` and `hitTest`. Pure |
| `scene.ts` | Where each square comes from and goes to, and when. Pure |
| `gl.ts` | The WebGL2 program: one instanced draw call, animated in the vertex shader |

**Load it lazily.** It is about 7 KB gzipped and only pages that draw it should fetch it. Import types with `import type`, which costs nothing, and the component with `lazy()` inside its own `<Suspense>`, so it never holds the page. Call `.preload()` when the page mounts, so the code downloads while the data does. `decode.ts` is small and may be imported statically.

```tsx
const TxGrid = lazy(() => import("../../components/txgrid/TxGrid"));

<div class="my-frame">{/* position: relative, with a size: the grid fills it */}
  <Suspense>
    <TxGrid
      txs={txs()}                       // GridTx[]: { id, vsize, fee, value?, rate?, coinbase? }
      capacity={1_000_000}              // vB the frame stands for; more than that shrinks to fit
      label="Map of the block's 4 198 transactions, …"   // the canvas's text alternative
      txid={(tx) => fetchApi(network(), `/block/${hash}/txid/${tx.id}`, { as: "text" })}
      highlight={rowHovered()}          // an id to ring, or undefined
      onState={setGridState}            // "drawn" | "lost" | "unsupported"
    />
  </Suspense>
</div>
```

**A whole block** is one array. **Changes** are a new array: the grid matches it to the last by `id`, so keep ids stable (the block index for a mined block, the txid for a projected one) and keep each unchanged transaction the same object (the grid caches its band per object). Squares that stay travel to their new places, new ones drop in, gone ones fade out where they stood, and a change that arrives mid-animation starts from where each square is. For a projected block (`projected-txs:N`, [`WS.md`](WS.md#binary-frames)), make each record `{ id: txid, vsize, fee, rate: effectiveRate / 1000 }`: `rate`, in sat/vB, orders and colors the square by the chunk it is mined in rather than by its own fee, and `value` is left out because the record does not carry it. `BlockTxs` in `lib/projected-txs.ts` keeps that map: apply the snapshot and each delta to it, and pass `[...model.txs.values()]`; order does not matter, because the layout sorts. Keep the grid mounted when the block empties, so the last squares fade out instead of vanishing. A transaction whose fee rate changed is a new object with the same id: it moves. Changes that arrive while the tab is hidden are applied without animation.

- **Layout.** Every square's side is its exact side (√(vsize ÷ vB per cell)) rounded to whole cells, at least 2; squares are placed first fit, row by row from the top left in fee order, then every square drops until it rests on the floor or another. The grid has about 160 000 cells, shaped to the frame in 4% steps, so a frame that changes size by a pixel keeps its picture and is only rescaled. If the squares overflow, they all shrink a little and are packed again. A full mainnet block (4 198 transactions) lays out in 5 to 8 ms in Chrome on an Apple M4, on the main thread.
- **Hit testing** is a lookup in the cell map, exact to the cell. It answers for where squares are going, not where an animation has them.
- **Colors and motion come from the tokens** at run time: `--fee-1` to `--fee-10`, `--color-text-muted` for the coinbase, the durations and curves. A theme switch repaints; reduced motion leaves only fades.
- **States.** `onState("drawn")` after the first frame; `"lost"` when the WebGL context is lost (the canvas hides; show your stand-in until `"drawn"` comes again on restore); `"unsupported"` when the browser has no WebGL2 (keep your stand-in and say why). Keep a stand-in in the frame until `"drawn"`.
- **Accessibility.** The canvas is `role="img"` with `label` as its name, and is not focusable. Put the same transactions in a list beside it and pass the hovered or focused row's id as `highlight`.
- **Measures.** It records `txgrid layout` and `txgrid first frame` with `performance.measure`, for profiling and browser tests.

## CSS

- Plain CSS with the tokens in `design/tokens.css`, imported once in `index.tsx`. Never copy a token value; use the custom property.
- Cascade layers, in this order: `reset, base, layout, components, pages`. Shared components write in `@layer components`, pages in `@layer pages`.
- One CSS file per component, next to it, imported by the component. BEM-style names: `block`, `block__part`, `block--variant`. Prefix a page's classes with the page name (`tx-flow`, `block-grid`) so pages never share a class by accident.
- Per-element values (a fee color, a position) are custom properties set through Solid's `style` object, with a value that comes from props or a signal: `style={{ "--swatch": feeColor(rate) }}`. Solid applies those through the CSSOM, which the Content-Security-Policy allows. Never write a `style` string, and never a `style` object whose values are all literals, because the compiler may turn it into an inline `style` attribute, which the policy blocks.
- Breakpoints: 1040 px and 640 px ([`DESIGN.md`](DESIGN.md)).

## Theme

Light by default, dark when the OS asks, either one forced by the toggle. A forced choice is stored under `theme` in `localStorage`. Choosing the scheme the OS already shows clears the stored choice, so the page follows the OS again.

The stored theme must apply before first paint, and the Content-Security-Policy forbids inline script. So `index.html` loads `public/boot.js` as a small blocking same-origin script in the head: it sets `data-theme` and `data-network` on `<html>` before the body is drawn. It is fetched alongside the render-blocking stylesheet, so it adds no round trip.

The font is preloaded, and `index.tsx` waits up to 1 s for it before the first render, so text is laid out once in Recursive. If it is not there in time, the app takes Recursive out of `--font-sans` and `--font-mono` for that page load (read from the tokens, not restated), so a late font never swaps in and reflows text; the next load finds it in the browser's cache. Anything that appears after the first render must not move what is already on screen: reserve its space, or put it where nothing follows it.

## Rules for page builders

1. **Stay in your directory.** A page owns `pages/<page>/` and nothing else. Its only edit outside is the one line in `src/routes.ts`, plus a function in `paths` and types in `lib/esplora.ts` when it needs them; those are append-only.
2. **Shared code changes in its own PR.** `components/`, `lib/`, `shell/` and `styles/` are shared. If a page needs a change there (a new prop, a fix, a new component), make it a small separate PR, or ask whoever is working on the shell. A component used by one page lives in that page's directory until a second page needs it.
3. **Chain data is text.** No `innerHTML`, no markup built from data, no data in `href` or `src` except through `paths` ([`SECURITY.md`](SECURITY.md) rule 14).
4. **No style attributes.** Custom properties through the `style` object with dynamic values only (see "CSS").
5. **Numbers through `lib/format.ts`.** Never `toFixed` a BTC amount or a fee rate in a page. Every amount, rate and size reaches a screen reader with its unit: in a table cell whose unit is in the header, use the components' `unit={false}`, or a `visually-hidden` unit after a number you format yourself.
6. **No layout shift.** Reserve space for what loads later.
7. **Stay in budget.** The shell and the home page together load at most 100 KB of gzipped JavaScript (`bun run size`). Other pages load on demand and should stay small too; a heavy library (WebGL, uPlot) loads only on the page that uses it.
8. **Test what has logic.** Pure functions get unit tests next to them. Components can be tested in Vitest's DOM: `mount()` in `components/test-utils.tsx` renders inside a router at a given URL.

## Checks

Lint, format and type checks are strict, and CI fails on any finding. OXC is the only linter and formatter: never add Biome, ESLint or Prettier.

| Command | Runs | Covers |
|---|---|---|
| `bun run typecheck` | `tsc` on `tsconfig.app.json`, then `tsconfig.node.json` | Everything: the app and its unit tests (browser types only, so app code cannot reach Node's `process` or `Buffer`); browser tests, scripts and configs (Node types) |
| `bun run lint` | `oxlint`, then `oxfmt --check` | What CI's lint step checks |
| `bun run format` | `oxfmt` | Rewrites files in place. `bun run format:check` checks without writing |

- **Lint.** Every `oxlint` category is an error (correctness, suspicious, perf, pedantic, style and restriction), with the TypeScript, unicorn, oxc, import, promise and JSX accessibility plugins, and Vitest's for unit tests. Rules that need types (unawaited promises, promises passed where a callback returns nothing, unsafe `any`) run through `oxlint-tsgolint` on the same tsconfig. Nothing is set to warn. A rule is off in `.oxlintrc.json` only with its reason beside it. An exception for one line is `// oxlint-disable-next-line <rule> -- <why>`; a disable that no longer matches anything fails the check.
- **Format.** `oxfmt` formats everything in `web/` it can parse: TypeScript, JavaScript, JSON, CSS and HTML, at 100 columns. It cannot format `scripts/e2e.sh`.
- **Types.** On top of `strict`: `noUncheckedIndexedAccess`, `exactOptionalPropertyTypes`, `noPropertyAccessFromIndexSignature` (an index signature is read as `x["key"]`, a reminder it may be missing), `noUnusedLocals`, `noUnusedParameters`, `noImplicitReturns`, `noImplicitOverride`, `noFallthroughCasesInSwitch`, `allowUnreachableCode` and `allowUnusedLabels` off, `noUncheckedSideEffectImports`, `erasableSyntaxOnly`, `verbatimModuleSyntax`, and `checkJs` for the JavaScript files. Left off: `isolatedDeclarations`, which is for publishing type declarations; and `skipLibCheck` stays on, so dependencies' own type errors are not ours to fix.

## Testing

`bun run test` runs Vitest in a happy-dom document, so logic and components are tested the same way. `bun run size` checks the first-load budget after `bun run build`: the scripts `index.html` loads, the home page's chunk, and everything they import statically. CI runs typecheck, lint, test, build and size, and the browser tests below ([`CI.md`](CI.md)).

## Browser tests

`bun run e2e` runs the Playwright tests in `web/e2e/` in headless Chromium against the real server and a private rbitcoin node on regtest ([`REGTEST.md`](REGTEST.md)). It needs Docker, `python3`, Rust and bun, and does everything else itself: starts the node and builds the known chain, builds the frontend and the server from your checkout, starts the server on a free port with only regtest enabled, runs the tests and removes all of it. A second run takes about 10 seconds. Every run uses its own node, port and output directory, so any number can run at once. On a failure it keeps the server log and Playwright's traces and prints where.

Arguments go to Playwright: `bun run e2e tx.e2e.ts`, `bun run e2e -g "deep link"`, `bun run e2e --headed`.

### Adding a test

Put it beside the others as `web/e2e/<page>.e2e.ts` (the `.e2e.ts` suffix keeps Vitest away from it), and import from `./fixtures`, never from `@playwright/test`:

```ts
import { chain, expect, test } from "./fixtures";

test("a block page shows its height", async ({ page }) => {
  await page.goto(`/regtest/block/${chain.blocks[106]}`);
  await expect(page.getByRole("heading", { level: 1 })).toContainText("106");
});
```

- **Pages are under `/regtest/`.** Only regtest is enabled, so the root is a 404.
- **`chain`** in `e2e/fixtures.ts` is the scenario's manifest: tip, block hashes, txids and addresses of every script type. They are the same on every run.
- **Every test fails on a console error, an uncaught exception or a Content-Security-Policy violation** on any page it opens. The fixture does this by itself. If your test causes an error on purpose, such as a request that is meant to 404, name it: `test.use({ consoleErrorsAllowed: [/status of 404/] })`. A CSP violation cannot be allowed.
- **Find elements by role and text,** as a reader would (`getByRole`, `getByText`), not by class names.
- **Tests share one node and run in parallel, so only `*.mine.e2e.ts` files mine.** A new block changes the tip every other test reads. Playwright runs those files in a separate `mines` project that starts after every other test has finished and runs one test at a time; only there does the `mine(n)` fixture work (it returns the new block hashes). Anything else that changes the chain belongs there too. The broadcast test in `api.e2e.ts` adds one transaction to the mempool, so do not assert the mempool's exact size.

  ```ts
  test("a new block lands without a reload", async ({ page, mine }) => {
    await page.goto("/regtest/");
    const [hash] = await mine(1);
    const strip = page.getByRole("list", { name: "Mined blocks, newest first" });
    await expect(strip.getByRole("link").first()).toHaveAttribute("href", `/regtest/block/${hash}`);
  });
  ```

- **Hostile chain data.** `hostile.mine.e2e.ts` puts markup, a `javascript:` URL, bidi overrides, ANSI escapes and NUL on the chain (`regtest.py hostile`: OP_RETURN outputs and a hand-built coinbase tag) and checks the transaction, block and address pages show them as visible, inert text: no injected element, no dialog, no unexpected link scheme. A new page that shows chain data belongs in it.
- **Keep the suite fast.** Browser tests cover what only a browser can show: that the page renders real data, links work and nothing errors. Logic belongs in unit tests.
