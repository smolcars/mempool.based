# Frontend

How the Solid 2 app in `web/` is put together (solid-js and @solidjs/web 2.0.0-rc.21, @solidjs/router 2.0.0-next.44, @solidjs/vite-plugin 3.0.0-next.57: pre-releases, pinned exactly), and the rules that let several people build pages at once without colliding. The visual rules are in [`DESIGN.md`](DESIGN.md); this file is about code.

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
      api.ts            HTTP client, and `createApi` and `createRequest` for components
      live.ts           The WebSocket client: `useTopic`, `useLiveStatus` (see "Live data")
      blocks-topic.ts   The `blocks` topic: its messages and how to keep a block list current
      projected-topic.ts The `projected` topic: every projected block's totals and the tip they sit on
      projected-txs.ts  The `projected-txs:N` topics: binary frames, and a block's transactions kept from them
      mempool-topics.ts The `mempool` and `transactions` topics: their messages, rates in sat/vB
      track-topics.ts   The `tx:<txid>` and `address:<address>` topics: following one transaction or address
      summary.ts        Block summaries: their type, a check, a page-wide cache and the batch fetch
      fakes.ts          Test stand-ins: a fake WebSocket and made-up blocks
      esplora.ts        Types for the Esplora replies pages use
      page.ts           `useNetwork`, `usePageTitle`
      clock.ts          Shared ticking clock for relative times
      difficulty.ts     The difficulty-adjustment reply and how it reads (the home and mining pages)
      config.ts         Reads `/app/config`
      chunks.ts         A page's code and styles that failed to download, and asking for them again
      theme.ts          Theme toggle state
    components/         Shared components, one CSS file each
      txgrid/           The transaction grid: decoder, layout, animation, WebGL2 renderer (see "Transaction grid")
      chart/            Time-series charts in SVG: scales, ticks, paths, the component (see "Charts")
      audit/            A block's audit: the block page's section, its list loaded on demand, the reader (see "Block audits")
    shell/              Header, network switch, theme toggle, band, footer, the frame around pages
    search/             What a query is (query.ts) and where it leads (search.ts); loaded on first use
    pages/<page>/       One directory per page: its routes, component, CSS and tests
```

## Routing

`@solidjs/router`. `index.tsx` makes the router with `createRouter({ routes, scrollRestoration: false })` (the browser restores scroll on back and forward) and renders the shell as its render-prop child. Mainnet pages are at the root and the other networks under `/signet`, `/testnet4` and `/regtest`, matching the server ([`ARCHITECTURE.md`](ARCHITECTURE.md)). `src/routes.ts` mounts every page's routes under all four prefixes, so a page is written once with network-free paths (`/tx/:txid`) and works on every network. The server sends the app shell for any page path, so deep links and reloads work; a disabled network's paths are a 404 from the server.

The network comes from the path. `useNetwork()` returns it; never keep it anywhere else.

| Page | Path | Directory | Built in |
|---|---|---|---|
| Home | `/` | `pages/home` | W2 |
| Block | `/block/:id` (hash or height) | `pages/block` | W3 |
| Blocks | `/blocks`, `/blocks/:from` (paging back from a height) | `pages/blocks` | W3 |
| Projected block | `/block/next`, `/block/next/:n` (2 to 8 places from now; 8 is the rest of the mempool) | `pages/projected` | W8 |
| Transaction | `/tx/:txid` | `pages/tx` | W4 |
| Address | `/address/:address` | `pages/address` | W5 |
| Mining | `/mining`, `/mining?period=24h\|1w\|1m\|1y\|all` | `pages/mining` | W9 |
| Mining pool | `/mining/pool/:slug` | `pages/mining` | W9 |
| Lightning | `/lightning`, `/lightning?interval=24h\|1w\|1m\|3m\|1y\|all` | `pages/lightning` | W11 |
| Lightning node, channel | `/lightning/node/:key`, `/lightning/channel/:id` | `pages/lightning` | W11 |
| Lightning search, rankings | `/lightning/search?q=`, `/lightning/rankings?by=capacity\|channels\|age` | `pages/lightning` | W11 |
| Mempool history | `/mempool`, `/mempool?interval=2h\|24h\|1w\|1m\|3m\|6m\|1y\|2y\|3y\|4y\|all` | `pages/mempool` | FE9 |
| Not found | anything else | `pages/not-found` | shell |

Every page is loaded lazily.

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

Internal links stay on the current network. Use `<Link href={paths.tx(txid)}>` from `components/Link.tsx`; it adds the prefix and the router handles the click. The router handles every same-origin `<a>` and marks the ones that match the location: `aria-current="page"` on an exact match (it never touches an `aria-current` you set) and `data-active` on a match by path. Give an in-page `#` link `rel="external"`, as the skip link has, so the browser scrolls to it and it is not announced as the current page. Build every path with `paths` in `lib/network.ts`, which encodes chain data into a single path segment so it can never change the route or the scheme ([`SECURITY.md`](SECURITY.md) rule 14). Add a function to `paths` for a new page rather than writing the path by hand. To navigate from code: `navigate(onNetwork(network(), paths.block(h)))`.

### Page title

Call `usePageTitle(() => "Block 975 411")`. Test networks get their name in front, and every title ends in `| rbitcoin.space`.

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

- `createApi<T>(path, { as: "text" })` returns `[request, { refetch }]`. `request()` is the answer: undefined before the first, the last one while it asks again, and it throws once the last attempt failed, so read `request.state` (`unresolved`, `pending`, `ready`, `refreshing`, `errored`) first where a failure is expected; `request.error`, `request.loading` and `request.latest` are there too. It asks again when the path or the network changes, aborts the request it replaces and the one in flight when the page unmounts, answers from the cache without a network request, and stays loading while a busy refusal waits to be retried. Return a falsy path to wait for something else first. `createRequest(source, (source, signal) => …)` is the same for a fetcher of your own (the block page's summary and transaction records).
- `fetchApi<T>(network, path, { as, signal })` is the same call as a promise, for event handlers and loops. `fetchApp<T>` is the same for the app's own API under the network's `/app`.
- **Chain-tip stamps.** `createApi<T>(path, { stamped: true })` and `fetchStamped<T>(network, path)` give `{ value, tip }`, where `tip` is the `X-Bitcoin-Chain-Tip` and its height that the reply is as of (undefined for an unstamped or cached reply). Take every figure that depends on the tip from one stamped reply, or compare the stamps of two: the block page counts confirmations from its status reply's stamp, and the home page's difficulty panel asks again while its answer's stamp trails the strip's tip.
- **Busy servers.** Every call above retries a 503 or 429 by itself, no sooner than its `Retry-After` (kept on `ApiError.retryAfter`), backing off from 1 to 30 s with a little jitter, six times, then rejects with the last error. They are all GETs, safe to repeat. Five 503s are not retried, because a retry cannot change them: an address too large to total, an address index that is off ([`API.md`](API.md#error-bodies-are-ours)), mining history on a server without a block table ([`API.md`](API.md#mining)), mempool history on a server without a data directory ([below](#mempool-history)), and the relatives and replacements routes on a network whose mempool engine is switched off. Show a loading state, not an error, while a call is pending. `withRetry(call, signal)` is the loop itself, for a call that does not go through `api.ts`; only wrap a call that is safe to repeat. A refusal it already gave up on is never retried by a loop around it. The browser still logs each refused request to the console; browser tests that may meet one allow `/status of (429|503)/`.
- **Coverage.** A history answer (the mining routes) says how far back its data reaches in `X-Data-From-Height`, `X-Data-From-Time` and `X-Data-Complete` ([`API.md`](API.md#coverage)). A stamped reply carries them as `coverage: { fromHeight, fromTime, complete }` when sent (`readCoverage` reads them). Say "data from … onward" where `complete` is false; never draw the missing stretch as zero.
- `as: "text"` reads plain-text replies (`/blocks/tip/height`, `/block-height/:h`, `/tx/:txid/hex`), and `as: "bytes"` binary ones as an `ArrayBuffer` (`fetchApp(n, "/block/<hash>/txs", { as: "bytes" })`). The default is JSON.
- Failures are `ApiError` with `status` (0 when the server cannot be reached, with a message that says so when the browser is offline) and a message fit to show. `isNotFound(e)` tests for 404. An abort is passed through as the abort error, not as an `ApiError`.
- Replies the server marks as never changing (`immutable`, or `max-age` of a day or more) are kept in memory, at most 256, for the life of the page. Everything else is fetched every time and left to the browser's HTTP cache.
- A request never suspends: the page frame shows at once and each section fills from `state` as its data arrives. Pages do not use Solid 2's async memos (`createMemo` returning a promise): read inside the shell's `<Loading>`, one would hold the whole page until it loads. The shell's `<Loading>` waits only for a page's code.
- An error thrown while rendering reaches the shell's `<Errored>` boundary, which shows "This page could not be shown". Catch expected failures (not found, node unavailable) in the page with `<ErrorState>` instead.

### When a page's files fail to load

Every page, and the transaction grid, is a chunk of code with its own stylesheet, downloaded on first use. A download can fail: a dropped connection, a busy proxy, or a deploy that replaced the files while a tab was open. Vite never asks for a failed stylesheet again by itself, so `lib/chunks.ts` notes every `<link rel="stylesheet">` and `<link rel="modulepreload">` (Vite preloads a page's code and its imports with one) that fails (`watchFiles`, started in `index.tsx`), and `retryFiles()` asks for each stylesheet again through a fresh `<link>` and for each script with a request that skips the cache, which proves it downloads now. The script itself is loaded again when its `import()` runs again, which the router's `lazy()` does on the next render. WebKit never does: once a script's preload or import failed, it answers every later `import()` of that script, and of any module that imports it, with the same failure and no request, for the life of the document (Playwright's WebKit 2370, checked against a 503 from a real server). Chromium and Firefox ask again.

The shell's `<Errored>` boundary handles both. On a chunk error it shows "Loading this page" and tries again after 1, 2 and 4 s: before each attempt it asks `/app/config` (never from a cache) whether the server's version changed since the tab loaded, then asks again for the failed files, and renders the page again only once they are in, so a page never appears unstyled. If the page fails to load again within 2 s of an attempt that proved its scripts download, the browser is holding on to the failure, and the page reloads (`reloadOnce`): that is how WebKit recovers. It reloads at most once a minute per tab, kept in session storage, so a file that downloads but never runs cannot loop; without session storage it does not reload, and the reader gets "Try again" as before. When the attempts run out it says that part of the page could not be downloaded, with "Try again", which asks for everything again. When the version changed, the files this tab needs were replaced by a deploy: it says the site has been updated and offers "Reload" rather than retrying files that are gone. A navigation starts the count afresh.

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
  "fee_bands": [367013, 9447, 153, 5069, 233233, 20986, 238267, 102460, 17310, 5453],
  "miner": {"name": "Foundry USA", "slug": "foundryusa"},
  "audit": {"match_rate": 99.87, "expected": {"txs": 3100, "vsize": 998000, "weight": 3991200, "fees": 1500000}, …}
}
```

- `tx_count` counts every transaction. Everything else leaves the coinbase out, as `getblockstats` does: `fees` in sats, `vsize` and `weight` summed over the paying transactions.
- `fee_rate` is in sat/vB, unrounded, `null` for a block holding only its coinbase. Percentiles are weighted by virtual size: `p25` is the rate of the transaction where the cheapest quarter of the block's vbytes ends. Print them with `formatFeeRate`.
- `miner` is who mined the block, by the pool list's rules ([`API.md`](API.md#who-mined-a-block)): a payout to a listed address first, then a listed tag, else `{"name": "Unknown", "slug": "unknown"}`. `name` is the list's spelling and `slug` its pool page (`paths.pool`); a coinbase tag only selects one of the list's names and is never itself the answer. It comes from the stored row's coinbase, or from the coinbase the summary's block listing already reads, so it costs no extra node call and a server without a block table names miners too. A server older than this field leaves it out: say the miner is not known.
- `audit` is the block's health: its audit's totals ([Block audits](#block-audits) below), or `null` when it has none. A new block's summary is computed with its audit, so the `summary` message and the first fetch already carry it; `null` stays `null`. A server older than this field leaves it out: treat that as `null`.
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

**In the frontend.** `lib/summary.ts` has the type, `isSummary`, and `fetchSummaries(network, hashes, signal)`, which asks for up to ten at a time with `withRetry` and keeps every summary it sees, fetched or pushed, for the life of the page (`knownSummary`). The home page asks for its ten tiles in one batch on first paint and then takes each new block's from the WebSocket, asking over HTTP only if the push has not come within 4 s. The blocks list asks per row, two at a time, so an old page fills in row by row. Draw the bands with `Strata`, and the records with the transaction grid below. The home strip's and the mining page's tiles put `miner.name` on the line under each tile (`aria-hidden`, its space always kept) and in the tile link's text; the block page links it to the pool's page.

**Caching.** A summary is fixed by its hash, so success is `Cache-Control: public, max-age=31536000, immutable`, and `api.ts`-style in-memory caching is right. Errors are `no-store`.

**Cost and limits.** A block in the block table with its fee rates ([`STORE.md`](STORE.md#blocks)) costs nothing: once backfilled, the facts of the last year's blocks come from there. `/txs` always needs the node. An uncached block costs the node a block listing: about 60 ms and 0.3 MB on mainnet where the node has the listing route, a heavy call (about 0.7 s and 10 MB) where it does not ([`RBITCOIN.md`](RBITCOIN.md#listing-a-blocks-transactions)), so:

- Concurrent requests for one block share one computation, and the facts and the records come from the same one.
- The newest ten blocks are computed as they arrive, so the home page's tiles are always cached. A new block's facts are also pushed on the WebSocket as a `summary` message; use that rather than fetching.
- Each network computes one block at a time, and holds at most six started by clients. Past that the answer is `503` with `Retry-After: 1`. Retry: the work already admitted goes on, so a retry finds more of it cached. A cold batch of ten needs a retry or two.
- Each client may start ten computations at once, then one every 2 s; past that `429` with `Retry-After: 2`. Cached blocks never count. A page walking back through history should fetch tiles lazily, as they scroll into view.
- `404` is an unknown block, `400` a malformed hash or query, `502` and `504` the node failing. All are worth a "Try again".

#### Block audits

How a block compared with the block we expected on its parent, for the block page's expected-against-mined view and a tile's health. The method and the meaning of each category are in [`MEMPOOL.md`](MEMPOOL.md#block-audits); read it before labelling anything. Per network, under its `/app` (`crates/server/src/audits.rs`).

| Route | Answer |
|---|---|
| `GET /app/block/:hash/audit` | The whole audit, JSON (below). `404` when the block has none, `503` on a server without a block table |

The totals, the same object as the facts' `audit` (which leaves out `id`, `height`, `time`, `seen_at`, `txs` and `complete`):

```json
{
  "id": "…", "height": 975540, "time": 1794480000, "seen_at": 1794480003,
  "match_rate": 99.87,
  "expected": {"txs": 3100, "vsize": 998000, "weight": 3991200, "fees": 1500000},
  "actual": {"txs": 3150, "vsize": 997500, "weight": 3990000, "fees": 1498000},
  "matched": {"txs": 2900, "vsize": 950000},
  "missing": {"txs": 3, "vsize": 1200}, "displaced": {"txs": 40, "vsize": 9000},
  "fresh": {"txs": 150, "vsize": 35000}, "conflicted": {"txs": 7, "vsize": 2000},
  "added": {"txs": 60, "vsize": 9500}, "unseen": {"txs": 1, "vsize": 200},
  "txs": [{"txid": "…", "kind": "missing", "vsize": 400, "rate": 1250, "first_seen": 1794479000}],
  "complete": true
}
```

- `match_rate` is percent with two decimals, rounded down: 100 means nothing was left out without an innocent cause. Only `missing` lowers it. Print it as is; do not round it up.
- `expected` is our first projected block on the parent; `actual` the mined block without its coinbase. `vsize` is sigop-adjusted on the expected side and `ceil(weight / 4)` on the mined side, so the two can differ by a little for the same transactions. Fees in sats.
- Each category is `{txs, vsize}`. `matched` + `missing` + `displaced` + `fresh` + `conflicted` is the expected block; `matched` + `added` + `unseen` the mined one.
- `txs` lists every transaction in a category other than `matched`, the most telling first (`missing`, `unseen`, `added`, `displaced`, `conflicted`, `fresh`), highest rate first within each, at most 1,000. `complete` is `false` when that cap cut the list. `kind` is the category. `rate` is sat per 1,000 vB: the rate we placed it at for the expected kinds, its own fee over its size for `added` and `unseen`. `first_seen` (Unix seconds) is our node's accept time, present only for expected kinds still in our mempool after the block. Matched transactions are not listed: draw them from the block's own records (`/txs`) and the txids from `/api/block/:hash/txids`.
- No audit is a state to show, not an error: "not audited: this server was not watching when it was mined" (or the projection was stale). It never appears later.

**In the frontend** (`components/audit/`). `model.ts` reads the totals with `readAudit(summary.audit)`, which gives `null` for `null`, a missing field (an older server) or a shape it cannot draw, and the detail route with `readAuditDetail`, which drops entries it cannot read. It also holds each category's name and plain-words meaning (`CATEGORIES`), the same words as [`MEMPOOL.md`](MEMPOOL.md#categories): use them wherever a category is named. `formatMatchRate` prints the score as sent, two decimals, never rounded up.

- **The block page** shows `AuditSection` below the map and facts, from the summary it already has: the match rate, the expected block against the mined one (transactions, virtual size, fees), the two laid side by side with their matched parts lined up, and every category with its count and size ("None" when empty). Without an audit it says so in one muted line, with no figures. A new block's facts can reach the page over the WebSocket before its own request answers, so while the facts are on their way the page listens to `blocks` and takes a `summary` for its block from there.
- **The list** (`AuditList.tsx`) is a lazy chunk, fetched with `/app/block/:hash/audit` only when the reader asks. It shows one category at a time, the server's most telling first, each transaction linked to its page, and says when the server cut it at 1,000 and how many of a category it lists.
- **The blocks list** has a "Match rate, %" column, blank for a block without an audit.
- **The grid is not tinted by category.** Its records have no txids, so marking audited transactions would mean fetching every txid of the block, and color in the grid carries fee and nothing else ([`DESIGN.md`](DESIGN.md#principles)). The list is the way in.

**Caching.** An audit is fixed once written: success is immutable, and a `404` is `no-store` but final in practice.

**Cost.** One indexed read, no node call; at most about 60 KB of JSON for a capped list.

#### A transaction's relatives and replacements

For the transaction page: who it will be mined with, and what replaced what. Per network, under its `/app` (`crates/server/src/relatives.rs`, `crates/server/src/replacements.rs`). Both answer from the server's memory and never ask the node, so they are cheap and need no caps beyond the ordinary request limits. Both are `Cache-Control: no-store`: the mempool moves.

| Route | Answer |
|---|---|
| `GET /app/tx/:txid/relatives` | A mempool transaction's cluster, its chunks and its parents and children (below) |
| `GET /app/tx/:txid/replacements` | What this transaction replaced and what replaced it, as far as the server remembers (below) |

**Relatives.** The cluster is the node's own (`getmempoolcluster`), as the projected blocks last asked for it: at most one mempool update behind. Which member spends which is read from the transactions' inputs.

```json
{"txid":"<parent>","stale":false,"chunk":0,
 "parents":[],"children":["<child>"],
 "chunks":[{"fee":3314,"vsize":265,"rate":12505,"txs":["<parent>","<child>"]}],
 "txs":[{"txid":"<parent>","chunk":0,"time":1794467650,"fee":139,"vsize":139,"block":0,"parents":[]},
        {"txid":"<child>","chunk":0,"time":1794467650,"fee":3175,"vsize":126,"block":0,"parents":["<parent>"]}]}
```

- `chunks` are the cluster's mining chunks in the node's order, best first; `chunk` is the index of the one holding this transaction. A chunk is mined whole, at its `rate`: that is the "effective fee rate" (CPFP). `fee` is the node's chunk fee, `prioritisetransaction` included; `vsize` is the chunk's sigop-adjusted size. A chunk can count a member the server has not seen yet, in the moment after it arrived, so `fee` and `vsize` can exceed the sum of `txs`.
- `txs` are the members in mining order with their own figures, the chunk they are in, their projected block (`null` before the projection places them) and their parents inside the cluster. `time`, `fee` and `vsize` are `null` for a member that left a moment ago.
- `parents` and `children` are this transaction's own unconfirmed ones. Their parents and children are in `txs`: draw the graph from there. A cluster holds at most 64 transactions.
- `404` `{"error":"not in the mempool"}`; `503` with `Retry-After: 1` for `mempool is syncing` (the server just started), `not placed yet` (it arrived a moment ago) or `mempool is switched off` (no retry will help); `400` for a malformed txid.

**Replacements.** A bounded record per network: the most recent `REPLACEMENTS_KEPT` (10,000) replacements, oldest out first, kept in memory and, with a `DATA_DIR`, in the store, so it survives a restart ([`MEMPOOL.md`](MEMPOOL.md#following-a-transaction)). Someone who opens a replaced transaction's page later still learns what replaced it.

```json
{"txid":"<B>","truncated":false,
 "replaced_by":[{"txid":"<B>","by":"<C>","seen":1794467700,"fee":695,"vsize":139,"rate":5000,"by_fee":2000,"by_vsize":139,"by_rate":14388}],
 "replaces":[{"txid":"<A>","by":"<B>","seen":1794467643,"fee":139,"vsize":139,"rate":1000,"by_fee":695,"by_vsize":139,"by_rate":5000}]}
```

- `replaced_by` follows this transaction forward to the newest replacement known, oldest link first. The last link's `by` is the one to show.
- `replaces` walks backward: what this one replaced, and what those replaced, newest first. A replacement can replace several transactions at once (a transaction and its children: a child leaves with its replaced parent and carries the same replacement).
- Each link: `seen` is when the server saw it (Unix seconds); `fee`, `vsize` and `rate` are the replaced transaction's own, `by_*` the replacement's, `null` when the replacement was confirmed in the block that removed the original. At most 32 links each way; `truncated` says there were more.
- Empty lists mean the server saw no replacement since it started, not that there was none. `503` with the mempool engine switched off; `400` for a malformed txid.

#### Mempool history

For the mempool chart: the mempool's size by fee band through the day, the week and the year, and what flowed through it. Per network, under its `/app` (`crates/server/src/statistics.rs`), from the server's history table, never the node ([`STORE.md`](STORE.md#mempool_history)). The public form in the reference's shape is `/api/v1/statistics/:interval` ([`API.md`](API.md#mempool-statistics)); this one keeps our ten bands, which the public one cannot.

| Route | Answer |
|---|---|
| `GET /app/mempool/history/:interval` | `interval` is `2h`, `24h`, `1w`, `1m`, `3m`, `6m`, `1y`, `2y`, `3y`, `4y` or `all`; anything else is `400` |

```json
{"interval":"2h","from":1794471360,"to":1794478503,"step":60,"first":1794477420,
 "minutes":119,"sampled":18,"complete":false,
 "points":[…,
  {"time":1794478500,"samples":1,"count":77664,"vsize":35034159,"fees":12561492,"min_fee_rate":100,
   "bands":[31473971,42955,44248,93275,1645692,162368,510336,793829,206603,60882],
   "in_count":111,"in_vsize":32392,"out_count":7,"out_vsize":2054,"mined_count":0,"mined_vsize":0}]}
```

A mainnet server 18 minutes after its first sample (keys are sent in another order).

- `points` are oldest first, one per `step` seconds (60 for `2h` and `24h`, 600 for `1w`, 3,600 for `1m`, up to whole days for `all`; never more than 1,460). `time` is the step's start, Unix seconds. **A step the server did not sample has no point**: draw a gap where consecutive `time`s differ by more than `step`, never a line across it and never a zero.
- Every value is the **mean over the minutes sampled in the point**, and `samples` says how many (at most `step / 60`). A point with fewer samples than that is partly covered; it is still a fair mean of what was seen.
- `count`, `vsize` (sigop-adjusted vB) and `fees` (sats) are the whole mempool. `bands` is the virtual size in each of the ten fee bands of [`DESIGN.md`](DESIGN.md#fee-scale), cheapest first, by effective rate: stack them, colored with the fee scale, and they sum to `vsize`. `min_fee_rate` is the node's minimum, sat per 1,000 vB like every rate the WebSocket sends (`100` is 0.1 sat/vB).
- The flow is per minute: `in_count` and `in_vsize` entered the mempool, `out_count` and `out_vsize` left it for any reason, and `mined_count` and `mined_vsize` are the part of what left that a block confirmed. Arrival in vB/s is `in_vsize / 60`. Over a minute, `vsize` moves by about what entered less what left (the totals come from the projected blocks, a moment behind the flow).
- **Coverage.** `first` is the oldest time stored (`null` with none): history begins when the server first ran with its mempool engine on. `from` and `to` are the period answered; `minutes` is how many whole minutes it owes and `sampled` how many it has; `complete` is `true` only when none is missing. The same is in `X-Data-From-Time` and `X-Data-Complete`. Say "since …" when `first` is after `from`, and show holes as holes.
- A new sample lands each minute, about a second after the minute turns; polling a chart every minute is plenty. The first minute after the server starts is never stored (its flow was not watched whole), so the first point appears 60 to 120 s after a start. `Cache-Control: public, max-age=1`. `503` with `Retry-After: 1` when the server has no data directory (`mempool history needs a data directory on this server`; no retry helps) or its store is busy; an empty `points` with `first: null` when it has stored nothing yet.

**In the frontend** (`pages/mempool/`): `history.ts` has the type, `isHistory`, the intervals, the coverage and flow sentences, and `seriesOf`, which turns an answer into the charts' arrays; the page and its tests are beside it.

- **The page is `/mempool`**, the interval in `?interval=` (24 hours by default), so a link reproduces the view. The home page's mempool section links to it.
- **Two charts.** The size, stacked by fee band in the fee colors with the total as a line on top, and the count, the node's minimum rate and (for steps over a minute) the minutes behind the point in the readout only (`notes`). Then the flow in vB a second: what arrived as a line over what blocks mined and what left otherwise as stacked layers, so a line above the layers is a filling mempool; a sentence above says which way the whole period went, weighted by the minutes behind each point.
- **Gaps** are the chart's `gap={step}`: neighbours more than one step apart are two runs. A lone minute is a sliver, not nothing. Before the step that holds `first` (its point is drawn at the step's start), the chart hatches the period, and a sentence says since when and how many of the period's minutes it holds; when recording began before the period, it says the gaps are times the server was not recording. `all` begins at the day holding `first` and owes only the minutes since it, so it always says since when, even when `complete`.
- **The live edge** is a refetch 3 s after each minute turns, not the `mempool` topic: the newest point is then exactly what the server stored, a mean of the minute, with no second model of it to keep in step. The last answer stays on screen, dimmed, while the next loads.
- **States:** loading; `503` without a data directory, said as such and not retried; nothing stored yet (the first minute appears by itself); nothing in this period, with the time records begin and a link to all of them.
- **Cost,** measured in Chromium at 4× CPU throttling on an Apple M4: 62 to 93 ms from the reply to the drawn chart for 1,415 points, 37 to 55 ms to redraw them on a width change, and the readout holds 60 fps while the pointer sweeps the chart.

#### Lightning pages

`pages/lightning/` reads the `/app/lightning/…` routes ([`API.md`](API.md#lightning)); `lightning.ts` has their types, shape checks, the close kinds' words and how sure each is (the words of [`LIGHTNING.md`](LIGHTNING.md#how-a-close-is-classified): use them wherever a kind is named), and the name cleaner. `testdata.ts` holds answers for tests, from the mainnet examples.

- **Status first.** Every Lightning page sits inside `Gate` (`parts.tsx`), which asks `/app/lightning/status` before anything else. That route always answers, so a network without Lightning costs one small request and no refusal: the other routes answer `503 lightning is off`, which the browser logs and `withRetry` would retry. `off` says Lightning is switched off on the network; `starting` and `syncing` show how far validation has come (channels checked, waiting, peers) and ask again every 5 s until `synced`, then draw the page without a reload; `failed` and `stopped` say the peer is not running. `synced` never goes back, so once seen the routes' index exists or is seconds away (its `503 … still syncing` is retried).
- **The overview** is `/app/lightning/overview` (one request: the snapshot, the top nodes, the newest opens and closes) and, beside it, the chart's series from `/app/lightning/statistics/:interval` (a week by default, the interval in `?interval=`). The chart is channels (area, left) and capacity (dashed, right), nodes and the median fee rate in the readout. A step without a snapshot is a gap (`gap={step}`); before the first snapshot it is hatched, and a sentence says since when. The series' `sampled` leaves out the ten minutes still running, so the page quotes it only for gaps inside a period it spans.
- **A node** is one request (`/app/lightning/nodes/:key`, its first page of open channels included); other pages and its closed channels are asked for as the reader moves. **A channel** is one request; its close names the kind, how sure that is, and why, and links the closing transaction and block.
- **Search** is the section's own field (on the overview and the search page), not the header's. The header's search leads one query to one page and works on every network; a name matches many nodes, so it needs a list, and the server's index answers it in one request (`/app/lightning/search`). The field refuses what the server would (under 3 or over 66 characters, control characters) without asking.
- **The transaction page's note** (`TxLightning.tsx`, the one import the transaction page takes from here) asks for a confirmed transaction only: the status, then `/app/lightning/tx/:txid` when it is `synced`. A network whose status said `off` is not asked again in that tab. The part that draws the note (`TxChannels.tsx`) is a lazy chunk loaded only when the answer lists a channel, and it sits after everything else on the page, so its arrival moves nothing.
- **Names are attacker-controlled text** ([`SECURITY.md`](SECURITY.md) rule 14). `cleanAlias` turns control characters and invisible format characters (bidi overrides and isolates, zero-width spaces; not the joiner emoji are built with) into U+FFFD, so a reader sees something is there, and thins stacked combining marks to two. `Alias` isolates the name in `<bdi>`, cuts it to 32 characters in lists (64 as a node page's title, with "Show the whole name"), counted as a reader counts them (`Intl.Segmenter`), and puts at most 512 in its `title`. The public key is always beside the name, and the node page says the key is the identity. Addresses are text chips, never links.
- **Node colours are left out.** A colour is the node operator's choice, color on this site carries fee and nothing else, and the Content-Security-Policy forbids a `style` attribute: there is nothing a colour would tell a reader that is worth a mapping onto classes.

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
- **A new block and the mempool topics arrive in a promised order** ([`WS.md`](WS.md#when-a-block-is-found)). For a new block, the `block` message comes first, then the mempool topics already on that block, sent at once rather than at the 2 s rate. No mempool message on an older tip follows the `block` message. The next `projected` after a `block` is on that block within milliseconds on regtest and about 0.6 s on mainnet, and the server sends an older one after a block's message only if its engine is still behind 3 s later; that message's `tip` says so. A transaction leaves the projection at the same moment, and a followed transaction's `tx:` topic says `confirmed` at the same moment, not at the next interval. `fees` on our own projection keeps the same order. So the home page draws a block as it is announced, crossing from the projection on its parent, and takes the next projection as the state after it; it holds back a projection on a block it has not shown (`HOLD_MS`, 3 s, `pages/home/projection.ts`) only while the strip catches up over HTTP after a gap or a reconnect, where the order does not reach: one `/blocks` request and, behind the server's one-second tip cache, one retry (`STALE_RETRY_MS`, 1.2 s, `pages/home/feeds.ts`). A new tile's facts are asked for over HTTP only when its `summary` has not come `PUSH_GRACE_MS` (4 s, `pages/home/summaries.ts`) after the block; on mainnet it came 1.6 to 3.4 s after it, audit included. The projected-block page waits at most `FOUND_WAIT_MS` (`pages/projected/state.ts`) for the block's new picture, which follows the projection at once, before saying the block was found.
- **The home page's fees** (`pages/home/fees.ts`) come from the `fees` topic alone while the socket is live: the levels the server gives, printed at the node's resolution and rounded up (`formatFeeRatePrecise`), the node's fallback (`source: "node"`) said so beside them, and `unavailable` shown as no number. While the socket is down, or when the topic has said nothing 5 s after the page opened, the page drops the topic's last answer and asks `/v1/fees/precise` every 30 s instead; that answer is ours when it carries a chain-tip stamp and the node's when it does not ([`API.md`](API.md#fees)).
- **The home page's mempool** (`pages/home/live-mempool.ts`) takes the panels and the strip from those topics and asks `/api` for nothing they send. It polls `/mempool` and `/mempool/recent` instead only when the server says `off`, while the socket is down, or when the topics have said nothing 5 s after the page opened (a server still making its first mempool cycle). `stale: true` keeps the last state on screen and turns the live indicator to "Node not answering".
- **Following one transaction or address:** `tx:<txid>` says where a transaction is and what happened to it (in the mempool with its projected block, replaced with the whole chain of replacements, confirmed, evicted, or unknown), and `address:<address>` sends a token that changes whenever the address's history does, so the page refetches. Each message is the whole state. The messages, what each state means for the page, how to phrase the position without false precision, and the bounds (four transactions and two addresses per socket) are in [`WS.md`](WS.md#following-a-transaction-or-an-address). On `unknown`, ask `GET /api/tx/:txid/status`: the topic never asks the node. On `refused` with `server_full`, fall back to polling the HTTP status and try the topic again later.
- **`lib/track-topics.ts`** reads both: `txTopic(txid)` gives `TxMessage` (a state with `stale`, or `off`, or `refused`), rates in sat/vB; `addressTopic(address)` gives `watching` with its status, `unavailable` or `refused` with a reason, and names the topic as the server does (bech32 in lower case). `linksOf` reads replacement links, which the topic and `/app/tx/:txid/replacements` share.
- **The transaction page's state** is a pure model in `pages/tx/state.ts`: the topic and the HTTP status route are two witnesses, and the newest word wins. A topic message decides only when it is live, not `stale` and not `unknown`; an HTTP "waiting" never hides the topic's position, since both agree; a 404 keeps what the topic or the replacement record knew about why it left. The page follows its transaction (and `blocks`, for confirmations) only while something can still change, and the newest replacement's topic when it was replaced: at most two `tx:` topics. A young confirmed transaction's first `unknown` asks nothing more; a later one, or one on a new block, asks the status route. An HTTP answer counts as of when it was asked, so a topic message heard while it was in flight wins. A live socket whose topic has said nothing for 5 s (a server still making its first mempool cycle) counts as not live, and the page checks over HTTP. A transaction the node did not know when the page opened is checked for until it appears.
- **An address's index can trail its status.** The node's Electrum status can change a moment before its Esplora routes show the change (seen on regtest), so the address page asks again after 1, 2 and 4 s while the refetched totals and first page are unchanged. Each of those refreshes also brings the balance chart up to date with one summary page ([below](#the-address-balance-chart)). The page fetches at once, before the node watches the address, so it fetches once more on the first status: nothing that arrived in between is missed. A change heard while a fetch is in flight restarts it. A later page that lands after a refresh changed the first page is dropped, since it may continue rows that are gone, and the page asks again after the rows the refresh left, so "Show 25 more" always shows more.
- **Say when it is not live.** While `status()` is not `live`, poll what the socket would have sent, no faster than every few seconds, and tell the reader (the home page's indicator).
- In tests, stub the global `WebSocket` with `FakeSocket` from `lib/fakes.ts` and play the server: `FakeSocket.last().hello()`, `.receive({...})`, `.drop(1006)`.

## Shared components

Import from `components/`. All of them render chain data as text only.

| Component | Use |
|---|---|
| `Amount sats unit? signed?` | BTC with eight decimals, trailing zeros dimmed, sats in the title. `signed` adds `+` and the positive color to incoming amounts. `unit={false}` hides the unit from sight only, for a table that names it in its header: screen readers still read it |
| `Sats sats unit?` | Grouped sats. `unit` as for `Amount` |
| `FeeRate rate unit? approx? precise?` | Fee rate with its band's swatch. `approx` adds `~`. `precise` prints a recommendation at the node's resolution, rounded up, and bands it as printed. `unit` as for `Amount` |
| `FeeKey` | The ten bands with their lower bounds. Put it next to every fee visual |
| `Strata layers settle? from?` | A block's fee bands as stacked layers, cheapest at the bottom, filling the box it is put in. Compute `layers` with `strataLayers(fee_bands, fill, min)`: `fill` is how full the block is, `min` the thinnest layer as a share of the box (about 2 px). `settle` drops the layers in, cheapest first; `from` grows each layer out of the same band's layer in another set (a projected block becoming the mined one), a band it lacks from nothing; reduced motion fades either. Decorative: give the figures, or `describeBands(fee_bands)`, as text beside it |
| `DifficultyAdjustment network tip` | The difficulty period's progress bar and the expected, next and previous change, refetched when `tip` (a block hash) changes. On regtest it says the network never retargets. The home and mining pages put it in their own section |
| `Chart …` | A time-series chart in SVG, in `components/chart/`. Load it with `lazy()`; see "Charts" |
| `AuditSection hash audit error? retry?` | A block's audit, in `components/audit/`: `audit` is the totals from `readAudit`, `null` for none, undefined while loading; its list loads lazily (see "Block audits") |
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
| `ErrorState title error? message? retry?` | What failed, the API's message (or `message`), and "Try again" |
| `Pagination page pageCount? hasNext? onPage label` | Previous and next. Esplora's cursor-paged lists pass `hasNext` instead of `pageCount` |
| `SearchField` | The search: reads the query as it is typed, opens its page on Enter or says why there is none. The header owns the one instance |
| `Link href` | Internal link on the current network |

Buttons are `<button class="btn">` (or `btn btn--accent`) with `components/Button.css`; toggles add `aria-pressed`.

For a figure without a component, use `lib/format.ts` directly: `formatBtc`, `formatSats`, `formatFeeRate`, `formatFeeRatePrecise` (a recommendation: three decimals, never rounded down), `feeBand` and `feeColor`, `formatSize`, `groupDigits`, `formatPowerOfTen` (a number too small or large to write out: `4.66 × 10⁻¹⁰`), `truncateMiddle`, `formatUtc`, `formatRelative`. They never throw and print `–` for a value that is missing or malformed. Fee bands always come from `feeBand`, which bands the rate as printed.

## Transaction grid

`components/txgrid/` draws transactions as packed squares in WebGL2: area by virtual size, color by fee band, the highest fee rate at the top left, resting on the floor of the frame like sediment ([`DESIGN.md`](DESIGN.md#the-transaction-grid)). The block page and the projected-block page use it.

| File | Holds |
|---|---|
| `TxGrid.tsx` | The component (default export, so `lazy()` can load it) |
| `decode.ts` | `decodeTxs(buffer)`: the `/txs` records as `GridTx[]`. Throws `TxsFormatError` (`kind: "version"` asks for a reload) |
| `layout.ts` | `GridTx`, `layout(txs, aspect, capacity)` and `hitTest`. Pure |
| `scene.ts` | Where each square comes from and goes to, and when. Pure |
| `gl.ts` | The WebGL2 program: one instanced draw call, animated in the vertex shader |

**Load it lazily.** It is about 7 KB gzipped and only pages that draw it should fetch it. Import types with `import type`, which costs nothing, and the component with `lazy()` inside its own `<Loading>`, so it never holds the page. Call `.preload()` in the page's `onSettled`, so the code downloads while the data does. `decode.ts` is small and may be imported statically.

```tsx
const TxGrid = lazy(() => import("../../components/txgrid/TxGrid"));

<div class="my-frame">{/* position: relative, with a size: the grid fills it */}
  <Loading>
    <TxGrid
      txs={txs()}                       // GridTx[]: { id, vsize, fee, value?, rate?, coinbase? }
      capacity={1_000_000}              // vB the frame stands for; more than that shrinks to fit
      label="Map of the block's 4 198 transactions, …"   // the canvas's text alternative
      txid={(tx) => fetchApi(network(), `/block/${hash}/txid/${tx.id}`, { as: "text" })}
      highlight={rowHovered()}          // an id to ring, or undefined
      onState={setGridState}            // "drawn" | "lost" | "unsupported"
    />
  </Loading>
</div>
```

**A whole block** is one array. **Changes** are a new array: the grid matches it to the last by `id`, so keep ids stable (the block index for a mined block, the txid for a projected one) and keep each unchanged transaction the same object (the grid caches its band per object). Squares that stay travel to their new places, new ones drop in, gone ones fade out where they stood, and a change that arrives mid-animation starts from where each square is. For a projected block (`projected-txs:N`, [`WS.md`](WS.md#binary-frames)), make each record `{ id: txid, vsize, fee, rate: effectiveRate / 1000 }`: `rate`, in sat/vB, orders and colors the square by the chunk it is mined in rather than by its own fee, and `value` is left out because the record does not carry it. `BlockTxs` in `lib/projected-txs.ts` keeps that map: apply the snapshot and each delta to it, and pass `[...model.txs.values()]`; order does not matter, because the layout sorts. Keep the grid mounted when the block empties, so the last squares fade out instead of vanishing. A transaction whose fee rate changed is a new object with the same id: it moves. Changes that arrive while the tab is hidden are applied without animation.

- **Layout.** Every square's side is its exact side (√(vsize ÷ vB per cell)) rounded to whole cells, at least 2; squares are placed first fit, row by row from the top left in fee order, then every square drops until it rests on the floor or another. The grid has about 160 000 cells, shaped to the frame in 4% steps, so a frame that changes size by a pixel keeps its picture and is only rescaled. If the squares overflow, they all shrink a little and are packed again. A full mainnet block (4 198 transactions) lays out in 5 to 8 ms in Chrome on an Apple M4, on the main thread.
- **Hit testing** is a lookup in the cell map, exact to the cell. It answers for where squares are going, not where an animation has them.
- **Colors and motion come from the tokens** at run time: `--fee-1` to `--fee-10`, `--color-text-muted` for the coinbase, the durations and curves. A theme switch repaints; reduced motion leaves only fades.
- **States.** `onState("drawn")` after the first frame; `"lost"` when the WebGL context is lost (the canvas hides; show your stand-in until `"drawn"` comes again on restore); `"unsupported"` when the browser has no WebGL2 (keep your stand-in and say why). Keep a stand-in in the frame until `"drawn"`.
- **Accessibility.** The canvas is `role="img"` with `label` as its name, and is not focusable. Put the same transactions in a list beside it and pass the hovered or focused row's id as `highlight`.
- **Measures.** It records `txgrid layout` and `txgrid first frame` with `performance.measure`, for profiling and browser tests.

## Charts

`components/chart/` draws the time series the mining, mempool-history and address-balance pages need, in SVG: a line, a filled area or a step on one value axis or two; percentile bands; and stacked shares. It is written here rather than taken from a library (the comparison with uPlot is in the PR that added it): about 4 KB gzipped, colors straight from the tokens, so a theme switch needs no repaint code.

| File | Holds |
|---|---|
| `Chart.tsx` | The component (default export, so `lazy()` can load it) |
| `scale.ts` | Linear and log scales, round value ticks, time ticks from hours to decades, `formatSi`, `formatHashrate`, `formatShare`, `formatDay`, `describeSeries`. Pure |
| `geometry.ts` | Paths broken at gaps, `stackLayout`, `nearestIndex` and `layerAt` for hit testing, `extent`, `sorted`. Pure |

**Load it lazily,** as the grid: `const Chart = lazy(() => import("../../components/chart/Chart"))`, call `Chart.preload()` in `onSettled`, and hold its height while it loads (`pages/mining/parts.tsx`, `ChartSlot`, holds `height` + 5.75rem: readout, plot and caption). Import types and the pure modules statically; they cost nothing until used.

```tsx
<Chart
  label="Hashrate and difficulty, the last year"   // its accessible name
  x={times}                                        // unix seconds, ascending
  domain={[yearAgo, now]}                          // optional: the time range drawn
  dataFrom={answer.coverage?.fromTime}             // optional: hatches what is not stored
  height={260}
  left={{ format: (v) => formatHashrate(v), log: "auto" }}
  right={{ format: (v) => formatSi(v, "") }}        // optional second scale
  series={[
    { name: "Hashrate", values: hashrates, draw: "area", format: (v) => formatHashrate(v, 4) },
    { name: "Difficulty", values: difficulty, draw: "step", axis: "right", tone: 2, format },
    // until: a step holds its last value on to this time (the address balance, to now)
  ]}
  bands={[{ name: "10th to 90th percentile", lo, hi, tone: 1, format }]}   // optional
  layers={[{ name: "Foundry USA", values: shares }, …]}                     // optional, 0–1
  layerFormat={formatShare}
  time={(t) => `Week of ${formatDay(t)}`}           // optional: how a point's time reads
  summary="…"                                       // optional: replaces the written summary
/>
```

- **Values** are numbers or `null`. A `null`, or two points more than three times the median step apart, breaks the line: missing data is a gap, never a dip to zero. Data with a fixed step passes `gap={step}`, so a single missing step breaks it too. A point alone between gaps is a short tick on a line; with `gap` given it is also a 2 px sliver in an area, which without it draws nothing (the mining charts, unchanged). Points before `dataFrom` do not exist; the part of `domain` before it is hatched and labelled "No data stored".
- **Axes.** `log: "auto"` turns a log scale on when the data spans more than three decades (hashrate over all of history). `zero` keeps 0 in view; `domain` fixes the range (`[0, 1]` for shares). `fee: true` runs the fee colors along a fee-rate axis, as its key. Ticks are round numbers; time ticks step by hours, days, Mondays, months or years, at most one per 84 px.
- **Tones,** never fee colors: tone 1 is the text color (the main series, solid), tone 2 the strong line color, dashed. Bands are the text color at 14% and 30%; stacked layers are shades of it with a seam of the page between them, the first at the bottom. A layer with `fee: n` (1–10) is amounts in fee band `n` instead, filled with that band's fee color (the mempool's size by fee rate), and can give the readout a `short` name (`0.2–0.3`) while `name` is what is read aloud and heads the table. The layer under the pointer turns the accent (interaction).
- **Notes** (`notes={[{ name, values, format }]}`) are figures the readout, `aria-valuetext` and the table show at each point without drawing them: a count beside a size.
- **The readout** sits above the plot at a fixed height and shows the latest point, or the one under the pointer or finger (a tap keeps it). The plot is a `slider`: it takes focus, the arrow keys, Page Up and Down, Home and End move through the points, Escape returns to the latest, and `aria-valuetext` reads the point's time and every figure.
- **Text alternative.** A summary written from the series (where each went, its lowest and highest) describes the slider, and "Show the figures" opens every point as a table. Pass `summary` when the series alone would not say it (stacked shares).
- **Cost.** It draws the paths again only when the data or its width changes, never on hover: 1.5 ms for a year of daily points and 5 ms for ten years, with a line, an area and eight stacked layers. Hovering a 967-point chart holds 60 fps.

### The address balance chart

`pages/address/BalanceChart.tsx` draws an address's confirmed balance after each block as a `step` series held on to now (`until`), with the block and its net change as notes; `pages/address/balance.ts` is the logic, pure but for `loadHistory`, which takes its fetcher. The data is the node's `/address/:address/txs/summary` ([`API.md`](API.md#routes)): 25 rows a page, newest first, each a confirmed transaction's net effect on the address (unconfirmed ones are not listed, so they stay in the page's "Unconfirmed" figure).

- **When it asks.** Only after the page's own totals and first history page have answered, and only when the totals count a confirmed transaction: an address with no history asks nothing and has no chart, and one whose totals the node refuses as too large asks nothing and says so in a sentence. That check matters: the node can spend a minute on the first summary page of the largest addresses (measured on mainnet's largest, which its totals refuse in 30 ms). The chart's code is preloaded at the same moment, so neither costs an ordinary page view anything before its main content is in. The section holds its height from the first render (the plot, the readout and the caption: three readout lines on a phone), so nothing moves when it arrives.
- **How much.** At most `MAX_PAGES` (8) summary pages, so the newest 200 transactions, every page after the first pinned with `?asof=` to the block the first is stamped with. A history that does not end within them is anchored by one more request, the address's totals as of the same block, and the balance is walked back from that; the header then says "The last 200 of 5 598 confirmed transactions" and the oldest block's change reads "no data" (its other rows may be on a page not loaded). A later page that fails is not an error: the pages already in are anchored the same way. So one page view makes at most 9 of the chart's requests, inside a client's share of the address routes ([`CONFIG.md`](CONFIG.md#address-and-batch-routes)).
- **Live.** The page's refresh on an address change also asks for the newest summary page, once. `advance` joins it to what is loaded by txid and block, carries the balance by what is new and what a reorg took away, and loads everything again only when the page does not meet the rows loaded. A change heard while a load is running queues one more update after it, never a second chain.
- **The axis** keeps zero in view for a whole history (it rose from nothing) and fits the data for a partial one, where zero would flatten every step into one line.

## CSS

- Plain CSS with the tokens in `design/tokens.css`, imported once in `index.tsx`. Never copy a token value; use the custom property.
- Cascade layers, in this order: `reset, base, layout, components, pages`. Shared components write in `@layer components`, pages in `@layer pages`.
- One CSS file per component, next to it, imported by the component. BEM-style names: `block`, `block__part`, `block--variant`. Prefix a page's classes with the page name (`tx-flow`, `block-grid`) so pages never share a class by accident.
- Per-element values (a fee color, a position) are custom properties set through Solid's `style` object, with a value that comes from props or a signal: `style={{ "--swatch": feeColor(rate) }}`. Solid applies those through the CSSOM, which the Content-Security-Policy allows. Never write a `style` string, and never put a literal value in a `style` object: Solid 2's compiler writes the literal entries of an object into the element's `style` attribute in its template, even beside dynamic ones, and the policy blocks that.
- Breakpoints: 1040 px and 640 px ([`DESIGN.md`](DESIGN.md)).

## Theme

Light by default, dark when the OS asks, either one forced by the toggle. A forced choice is stored under `theme` in `localStorage`. Choosing the scheme the OS already shows clears the stored choice, so the page follows the OS again.

The stored theme must apply before first paint, and the Content-Security-Policy forbids inline script. So `index.html` loads `public/boot.js` as a small blocking same-origin script in the head: it sets `data-theme` and `data-network` on `<html>` before the body is drawn. It is fetched alongside the render-blocking stylesheet, so it adds no round trip.

The font is preloaded, and `index.tsx` waits up to 1 s for it before the first render, so text is laid out once in Recursive. If it is not there in time, the app takes Recursive out of `--font-sans` and `--font-mono` for that page load (read from the tokens, not restated), so a late font never swaps in and reflows text; the next load finds it in the browser's cache. Anything that appears after the first render must not move what is already on screen: reserve its space, or put it where nothing follows it.

## Rules for page builders

1. **Stay in your directory.** A page owns `pages/<page>/` and nothing else. Its only edit outside is the one line in `src/routes.ts`, plus a function in `paths` and types in `lib/esplora.ts` when it needs them; those are append-only.
2. **Shared code changes in its own PR.** `components/`, `lib/`, `shell/` and `styles/` are shared. If a page needs a change there (a new prop, a fix, a new component), make it a small separate PR, or ask whoever is working on the shell. A component used by one page lives in that page's directory until a second page needs it.
3. **Chain data is text.** No `innerHTML`, no markup built from data, no data in `href` or `src` except through `paths` ([`SECURITY.md`](SECURITY.md) rule 14).
4. **No style attributes.** Custom properties through the `style` object, every value dynamic (see "CSS").
5. **Numbers through `lib/format.ts`.** Never `toFixed` a BTC amount or a fee rate in a page. Every amount, rate and size reaches a screen reader with its unit: in a table cell whose unit is in the header, use the components' `unit={false}`, or a `visually-hidden` unit after a number you format yourself.
6. **No layout shift.** Reserve space for what loads later.
7. **Stay in budget.** The shell and the home page together load at most 100 KB of gzipped JavaScript (`bun run size`). Other pages load on demand and should stay small too; a heavy component (the WebGL grid, the charts) loads only on the page that uses it.
8. **Plain HTTP is a real deployment.** A self-hosted explorer is often reached over HTTP at a private address, where browsers hide APIs they keep for secure contexts (`navigator.clipboard`, `crypto.subtle`, `crypto.randomUUID`, service workers). Code that uses one has a fallback that works without it, and a browser test from an insecure origin (`e2e/copy.e2e.ts` shows how); `CopyButton` falls back to the copy command and says when nothing was copied.
9. **Test what has logic.** Pure functions get unit tests next to them. Components can be tested in Vitest's DOM: `mount()` in `components/test-utils.tsx` renders inside a router (`createRouter` with `memoryHistory(url)`) at a given URL.
10. **Write for Solid 2.** Its migration guide and RFCs are in the solid repository under `documentation/solid-2.0/`; these are the rules that bite in this app:
   - **A write is seen after the flush.** `setX(v); x()` still gives the old value until the update is applied (the next microtask, or `flush()`). Code that writes and then reads in the same turn keeps the value in a plain variable that the signal mirrors (`createBlockFeed`'s list, the pool page's count), or uses the updater form `setX((n) => n + 1)`. `batch` is gone: writes in one turn are applied together.
   - **Effects are split.** `createEffect(() => source(), (value, previous) => { …; return cleanup })`: the first function tracks and only reads, the second does the work untracked and returns its cleanup. `on()` is gone: the first function is the dependency list, and `{ defer: true }` skips the first run. The first function runs when the effect is created, so declare an effect below everything it reads.
   - **`onSettled` replaces `onMount`.** It returns its cleanup; `onCleanup` inside it throws. It runs once, untracked.
   - **No writes from a component body or a memo** (development throws). Write in handlers, effects' second function, `onSettled`, or async callbacks. A value derived from another and also written (the address history's later pages) is a function-form `createSignal((prev) => …)`; `createComputed` is gone.
   - **Boolean attributes are presence.** `true` writes an empty attribute and `false` removes it, so `aria-*` and other true/false attributes take the strings: `aria-busy={loading() ? "true" : "false"}`, `spellcheck="false"`. Attribute names are lowercase (`maxlength`, `tabindex`).
   - **Classes and lists.** `classList` is gone: `class={["base", { "base--on": on() }]}`. `Index` is `<For keyed={false}>`, `splitProps` is `omit`, `Suspense` is `Loading`, `ErrorBoundary` is `Errored`.
   - **Read props in JSX or a memo.** A read at the top of a component body is not tracked and warns in development; wrap a deliberate one-time read in `untrack`.
   - **In tests, flush before you look.** After an event, a timer or a write the test made, call `flush()` before reading signals or the DOM. `FakeSocket` flushes after each message and close, as a browser's task boundary would.

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

`bun run test` runs Vitest in a happy-dom document, so logic and components are tested the same way. `bun run size` checks the first-load budget after `bun run build`: the scripts `index.html` loads, the home page's chunk, and everything they import statically. `vite.config.ts` groups `live.ts`, `blocks-topic.ts`, `projected-topic.ts` and what they import (Solid's runtime among it) into one `app` chunk: the transaction and address pages share only part of it, which would otherwise split the first load into several small files that each cost gzip overhead. CI runs typecheck, lint, test, build and size, and the browser tests below ([`CI.md`](CI.md)).

## Browser tests

`bun run e2e` runs the Playwright tests in `web/e2e/` in headless Chromium ([or another engine](#other-engines)) against the real server and a private rbitcoin node on regtest ([`REGTEST.md`](REGTEST.md)). It needs Docker, `python3`, Rust and bun, and does everything else itself: starts the node and builds the known chain, builds the frontend and the server from your checkout, starts the server on a free port with only regtest enabled and its block table in the run's directory (with some mempool history already in it, [below](#adding-a-test)), runs the tests and removes all of it. A second run takes about 10 seconds. Every run uses its own node, port and output directory, so any number can run at once. On a failure it keeps the server log and Playwright's traces and prints where.

Arguments go to Playwright: `bun run e2e tx.e2e.ts`, `bun run e2e -g "deep link"`, `bun run e2e --headed`.

**The server runs with every limit at its production default** ([`CONFIG.md`](CONFIG.md#limits)), so each run also proves that real page loads stay inside them. The tests all come from one address, so each test poses as its own visitor, as its fresh browser context (empty cache, no cookies) is: the server trusts `X-Forwarded-For` from `127.0.0.1` only, and the fixture sends an address of the test's own (`198.18.<worker>.<test>`) on every page, file, API call, WebSocket handshake and `request` call. One address per worker would be one person making a dozen cold loads in seconds, which the block summary route's per-client limit rightly refuses. A test that sets `extraHTTPHeaders` itself must keep that header.

**A limit that refuses anything fails the run, and says which.** A page or static file answered `429` or `503` ends its test at once with the request named, instead of a later assertion timing out. After the tests, `bun run e2e` fails if the server's log holds any refusal, even of an API call a page retried, and prints them: each line names the route, the limit (`refused`) and the worker's address (`client`).

### Other engines

```sh
E2E_ENGINE=webkit bun run e2e              # or firefox; chromium is the default
E2E_ENGINE=webkit E2E_PHONE=1 bun run e2e  # at a phone's width
```

`E2E_ENGINE` picks Playwright's Chromium, WebKit or Firefox, and `E2E_PHONE=1` a phone: Chromium as a Pixel 7, WebKit as an iPhone 15 (mobile viewport, touch, three device pixels per CSS pixel), and Firefox, which has no mobile emulation, as a 393 × 659 window with touch and three device pixels per CSS pixel. The script downloads the engine into Playwright's own cache on first use; on Linux, `node_modules/.bin/playwright install-deps <engine>` (as root) adds its system libraries once. Pull requests run Chromium at desktop width only; the weekly Browsers workflow runs WebKit and Firefox at both widths ([`CI.md`](CI.md#other-browsers)).

What it proves: the pages work in the WebKit and Gecko engines. What it does not: Playwright builds its own WebKit, so a pass is not Safari on a Mac or an iPhone (other graphics, fonts, network stack and settings), least of all on CI's Linux. Check a change that depends on the engine by hand in Safari too.

Differences found so far (2026-11, Playwright 1.64):

- **Closing a socket that is still connecting** is logged as a failed connection by WebKit and Firefox, not by Chromium. Leaving a page often does that, so `lib/live.ts` closes such a socket once it opens instead.
- **A page whose script failed to download** cannot be loaded again in the same document in WebKit, which keeps the failure ([above](#when-a-pages-files-fail-to-load)). WebKit recovers by reloading the page, Chromium and Firefox in place; `chunks.e2e.ts` covers all three.
- **A character the font lacks** is drawn from another font, chosen differently by each engine: the narrow no-break space that groups digits was 0.15 em in Chromium, 0.12 em in Firefox and 0.05 em in WebKit (`1 293` read as `1293`) until it was added to the subset ([`DESIGN.md`](DESIGN.md#type)). Give any new character a glyph in the subset; `shell.e2e.ts` measures the digit-group gap in every engine.
- **On Linux** (the Browsers workflow, [`CI.md`](CI.md#other-browsers)), Firefox without a display has no WebGL at all ("Exhausted GL driver options"), so the workflow runs it under Xvfb, where Mesa draws. WebKit there shows a WebGL frame drawn without antialiasing only once the next frame is drawn, so the grid draws its last frame twice; otherwise a single frame (a restored context, a resize, a theme switch) never appears.
- **Safari's Tab** reaches only form fields unless the reader has turned on "Press Tab to highlight each item"; Option-Tab reaches the skip link and every link. Playwright's WebKit does the same.
- **WebKit shows a JSON reply** (`/healthz`, an API route opened as a page) in a viewer the Content-Security-Policy refuses, which logs a violation that is not the site's.
- Same in all three: `ws:` and `wss:` to the page's own host under `default-src 'self'`, the `Origin` check on `/app/ws`, WebGL2 and the page without it, the charts' pointer, touch and keyboard read-outs, reduced motion. Numbers and dates are formatted by our own code, not `Intl`, so they read the same everywhere.

Write browser tests so they hold in every engine:

- **Read a screenshot at the device's scale.** Desktop Safari has two device pixels per CSS pixel and the iPhone three: divide by the ratio, never assume 1 (`grid.e2e.ts`).
- **CSP violations are reported by the page,** through `securitypolicyviolation`, since each engine words its console message differently. In WebKit, every Playwright screenshot adds a style the policy refuses, reported against the document rather than an element; the fixture names that one and drops it. `csp.e2e.ts` proves a style of the site's still fails its test, screenshot or not.
- **A test that depends on width checks it** (`page.viewportSize()`), as the home page's test does for the note a phone hides.
- **Phone screenshots for a UI change come from WebKit** as an iPhone, besides Chromium: the owner's first device is a phone.

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
- **Wait on the page's own bounds, by name.** The projected blocks can trail the node by up to `PROJECTION_LAG_MS` (in `e2e/fixtures.ts`, with its reasoning): a new transaction until the tracker's next cycle and the topic's next send, a new block until its projection, which follows the `block` message at once unless the node is slow to serve the block; the next block's page then holds its notice for up to `FOUND_WAIT_MS` (`src/pages/projected/state.ts`). Import those constants rather than writing a timeout, so the test and the page cannot drift apart. Each wait ends as soon as the page catches up. Before a mining test starts, the `mine` fixture waits until the server's `blocks` and `projected` topics both sit on the node's tip, at most `PROJECTION_LAG_MS`, and fails naming what lagged; otherwise blocks an earlier test mined would arrive together with this test's.
- **Mempool history is stored before the server starts.** The server records its first minute 60 to 120 s after it starts, so `bun run e2e` first writes 15 minutes an hour old, with a five-minute gap, through the sampler's own writer (`crates/engine/examples/history_seed.rs`), and `mempool.e2e.ts` checks the page shows exactly those. Never wait for a live minute in a browser test: the sampler's timing, skipped minutes and roll-ups are proved by the engine's tests on a paused clock (`crates/engine/src/history/tests.rs`), and the page's refetch at each minute by its unit tests.
- **Pages regtest cannot fill** (Lightning is off on regtest) are driven from recorded answers: `lightning.e2e.ts` serves `/regtest/app/lightning/…` with `page.route` from mainnet answers in `fixtures/lightning/app/`, so the built pages are drawn from real data with no test-only route on the server. Its own status route still answers `off`, which the same file checks.
- **Keep the suite fast.** Browser tests cover what only a browser can show: that the page renders real data, links work and nothing errors. Logic belongs in unit tests.
