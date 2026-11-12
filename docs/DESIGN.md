# Design

The design language for rbitcoin.space. The tokens live in [`design/tokens.css`](../design/tokens.css). A static prototype of the home, block and transaction pages is in [`design/prototype/`](../design/prototype/).

## Goal

A block explorer that is faster and better looking than mempool.space, with motion that makes live data easy to follow.

## Idea: sediment

Transactions settle out of the mempool into blocks the way sediment settles into layers. The whole identity follows from that:

- **A block is a core sample.** Every block tile, mined or projected, shows its fee distribution as stacked layers (strata): cheapest at the bottom, dearest on top, each layer as thick as the virtual size it holds. You can compare blocks at a glance without opening them.
- **Fee rate is temperature.** Cheap is cold (blue), expensive is hot (red), through teal, green and ochre. The scale has real resolution between 0.1 and 5 sat/vB, where almost all traffic now sits.
- **Mined is solid, projected is provisional.** Mined tiles are filled and outlined. Projected tiles are dashed and transparent. When a block is found, the next projected tile crosses the "now" line and sets.
- **Palette:** limestone (light) and basalt (dark), cool green-grey neutrals with one accent, rhodonite pink. The accent is never a fee color, so it never carries meaning about fees.

## Principles

1. **Numbers are the content.** Every figure uses tabular digits. Amounts, rates and sizes are aligned, grouped and never squeezed. Decoration never competes with a number.
2. **Color carries fee and nothing else.** The fee scale is reserved for fee rate. Status uses the positive, caution and negative tokens, always with an icon and a word. The accent marks interaction: the primary button, focus, the current page, the block that just landed.
3. **Never color alone.** A fee color is always next to its number. A status color always has an icon and a word.
4. **Motion explains a change.** Something moves only when the data changed or the user acted. One orchestrated moment per event, never ambient decoration.
5. **The network is never ambiguous.** A test network shows a hatched band on every page, a recolored logo and a recolored network switch.
6. **Chain data is text.** Transaction data, coinbase tags, OP_RETURN text and Lightning aliases are attacker-controlled. They are set with `textContent` and never parsed as HTML ([`SECURITY.md`](SECURITY.md) rule 14). The prototype builds every node with one helper that only creates text nodes.
7. **Sentence case everywhere.** No all-caps labels and no decorative eyebrows above headings.

## What to keep from the genre

The ideas people already understand: blocks as tiles in a strip, with projected blocks on the left and mined blocks on the right, and a block's transactions drawn as a packed grid of squares sized by virtual size.

## What is our own

The visual identity: logo, color, type, spacing, motion, the strata inside tiles, the fee ladder, the "now" line, the network band. Nothing is copied from mempool.space, and its source code is never consulted.

## Constraints

| Constraint | Why |
|---|---|
| No third-party requests: fonts, icons and scripts are self-hosted | Privacy, and the Content-Security-Policy is `default-src 'self'` |
| Light and dark themes | Both are first-class |
| Works at phone width | No horizontal page scroll |
| WCAG AA contrast | Fee colors included |
| Motion respects `prefers-reduced-motion` | Accessibility |
| Animations hold 60 fps | Use transform and opacity; heavy scenes are WebGL |
| Design tokens are CSS custom properties, with no CSS-in-JS runtime | Performance budget in [`ARCHITECTURE.md`](ARCHITECTURE.md) |
| Chain data is always rendered as text | [`SECURITY.md`](SECURITY.md) |

## Color

Light is the default. Dark follows the OS, and `<html data-theme="light">` or `data-theme="dark"` forces either. The dark values appear twice in `tokens.css`, once for the OS setting and once for the forced choice. Keep the two blocks identical.

| Token | Light | Dark | Use |
|---|---|---|---|
| `--color-bg` | `#e7ece9` | `#0e1b1d` | Page background |
| `--color-surface` | `#fbfcfb` | `#142527` | Header, mined tiles, panels |
| `--color-surface-raised` | `#ffffff` | `#1b3033` | Tooltips, menus |
| `--color-surface-sunken` | `#dce3df` | `#0a1517` | Block strip band, inputs, grid frame |
| `--color-line` | `#c7d2ce` | `#26403f` | Hairlines between rows |
| `--color-line-strong` | `#687d78` | `#628386` | Control borders, projected tile outline (3:1 on every surface) |
| `--color-text` | `#10282a` | `#e3ece7` | Body text, section rules |
| `--color-text-muted` | `#4b625f` | `#93aaa4` | Labels, units, dimmed zeros (4.5:1 on every surface) |
| `--color-accent` | `#a8196b` | `#f27ab9` | Primary button, focus ring, current page, landing block |
| `--color-accent-soft` | `#f6dcea` | `#3a1f30` | Highlight behind new rows, pressed toggles |
| `--color-on-accent` | `#ffffff` | `#1a0a12` | Text on the accent |
| `--color-focus` | accent | accent | Focus outline |
| `--color-positive` | `#1b6e43` | `#5fd39a` | Confirmed |
| `--color-caution` | `#8a5a00` | `#f0b43c` | Unconfirmed, waiting |
| `--color-negative` | `#b42318` | `#ff8a7a` | Replaced, failed, errors |

All text tokens reach 4.5:1 and `--color-line-strong` reaches 3:1 against `bg`, `surface`, `surface-raised` and `surface-sunken` in both themes. The prototype check measures this. If you add a surface, measure again.

### Fee scale

Ten bands. Band thresholds are in sat/vB, lower bound inclusive.

| Token | Band | Light | Dark |
|---|---|---|---|
| `--fee-1` | under 0.2 | `#4f6dcc` | `#6581d4` |
| `--fee-2` | 0.2 to 0.3 | `#0077c3` | `#3f96d9` |
| `--fee-3` | 0.3 to 0.5 | `#0080ad` | `#00a9d1` |
| `--fee-4` | 0.5 to 1 | `#008688` | `#0ab8b9` |
| `--fee-5` | 1 to 1.5 | `#008555` | `#37c695` |
| `--fee-6` | 1.5 to 2 | `#467a00` | `#92c851` |
| `--fee-7` | 2 to 3 | `#866100` | `#debb19` |
| `--fee-8` | 3 to 5 | `#9e4b00` | `#f69900` |
| `--fee-9` | 5 to 10 | `#aa3600` | `#fa6e1d` |
| `--fee-10` | 10 and up | `#ab2721` | `#f54748` |

- The thresholds are a frontend constant (`FEE_BOUNDS` in the prototype's `common.js`). If fee levels move, change the thresholds, not the colors.
- Band the rate as printed, not the raw value: 0.1996 sat/vB prints as `0.20` and takes band 2, so the color never disagrees with the number next to it.
- Internal links keep the selected network. In the app the network is the path prefix; the prototype carries it as `?network=`.
- Every fee color reaches 3:1 against every surface in both themes, which WCAG requires for graphics. Most reach 4.5:1. Fee colors are fills: squares, strata, swatches and pins. They are never used as text color.
- In light mode the scale is darker and more saturated so it holds contrast on white. In dark mode it brightens toward the hot end, so dear transactions glow.
- The **fee key** (ten swatches with their lower bounds) appears next to every fee visual.

### Network

`<html data-network="mainnet|signet|testnet4|regtest">` sets `--color-network`.

| Token | Light | Dark |
|---|---|---|
| `--network-mainnet` | `#10282a` | `#e3ece7` |
| `--network-signet` | `#6d3fd8` | `#b49cff` |
| `--network-testnet4` | `#8a5a00` | `#f0b43c` |
| `--network-regtest` | `#4b625f` | `#93aaa4` |

Mainnet shows its name in the network switch and nothing else. Test networks add three signals: a full-width hatched band above the header ("Signet is a test network. Coins here have no value."), the logo tile in the network color, and the network switch outlined in the network color. The page title is prefixed with the network name.

## Type

**One family: [Recursive](https://github.com/arrowtype/recursive)** (SIL Open Font License 1.1, license in `design/fonts/OFL.txt`). It has a monospace axis, so hashes and addresses come from the same file as the interface text and sit in the same rhythm.

- **One file, 38 KB:** `design/fonts/recursive.woff2`, a variable font with `wght` 300–800 and `MONO` 0–1. Casual, slant and cursive axes are pinned off, and the character set is Basic Latin plus the punctuation we use (–, —, …, ≈, ≥, ×, arrows, ₿, superscript digits and minus, thin and narrow no-break spaces). Recursive has no narrow no-break space of its own, so the build adds one, 0.15 em wide at every weight: without it each browser drew another font's space between digit groups, and WebKit's was too thin to see.
- If the font fails, the stack falls back to the system UI font and system monospace.
- **Turn on monospace** with `font-family: var(--font-mono); font-variation-settings: var(--mono-on);`. Use it for hashes, txids, addresses and scripts only. Numbers stay proportional with tabular digits.
- Rebuild the subset from the upstream release (`Recursive_VF_1.085.ttf` in `ArrowType-Recursive-1.085.zip`) with fontTools 4.68 and `brotli`, from `design/fonts/`. With the same versions this reproduces the file except its timestamp:

```sh
fonttools varLib.instancer Recursive_VF_1.085.ttf CASL=0 slnt=0 CRSV=0 wght=300:800 -o rec.ttf
python3 add-nnbsp.py rec.ttf
pyftsubset rec.ttf --flavor=woff2 --output-file=recursive.woff2 \
  --unicodes="U+0020-007E,U+00A0,U+00B0,U+00B2,U+00B3,U+00B7,U+00B9,U+00D7,U+2009,U+2013,U+2014,U+2018,U+2019,U+201C,U+201D,U+2026,U+2070,U+2074-2079,U+207B,U+2190-2193,U+2197,U+2212,U+2248,U+2264,U+2265,U+202F,U+20BF" \
  --layout-features='kern,liga,calt,tnum,pnum,zero,case,frac,ss01,ss02,ss03,ss04,ss05,ss06,ss07,ss08,ss09,ss10,ss11,ss12'
```

Body text turns on stylistic sets `ss01`, `ss02` and `ss08` (simpler a, g and l) for legibility.

### Scale

A minor-third scale on a 16 px base.

| Token | Size | Use |
|---|---|---|
| `--text-xs` | 12 | Tile footers, axis ticks, fee key |
| `--text-sm` | 14 | Labels, tables, captions, controls |
| `--text-md` | 16 | Body |
| `--text-lg` | 19 | Section headings, tile heights, wordmark |
| `--text-xl` | 23 | Key stats |
| `--text-2xl` | 28 | Reserved |
| `--text-3xl` | 28–42, fluid | Page titles |

Weights: `--weight-light` 340 (dimmed zeros), `--weight-regular` 420, `--weight-medium` 540, `--weight-bold` 700, `--weight-heavy` 790 (page titles, block heights, wordmark). Line heights: `--leading-tight` 1.15 for headings and figures, `--leading-snug` 1.3, `--leading-body` 1.5. Headings use `--tracking-tight`.

### Numbers

| Kind | Treatment | Example |
|---|---|---|
| Any figure | `font-variant-numeric: tabular-nums slashed-zero` (class `num`) | |
| Grouping | Narrow no-break space (U+202F) every three digits, so numbers never wrap | `975 411`, `3 341 tx` |
| BTC | Always eight decimals. Trailing zeros in muted color at light weight, so the significant part reads first. Hover shows sats | `0.03500000`, with `00000` dimmed |
| sats | Grouped integer | `167 sats` |
| Fee rate | Two decimals under 10, one under 100, none above. Always with a fee swatch | `0.80 sat/vB` |
| Size | One quantity, one unit family, everywhere: serialized size in B, kB, MB; virtual size in vB, kvB, MvB; weight in WU, kWU, MWU. The base unit under 10 000, then k or M (powers of 1 000) with two decimals (`formatSize`). A block's **size** is its serialized size; its virtual size is always written in vB units or labelled "virtual". The same block reads the same string on every page | `209 vB`, `1.69 MB`, `998.52 kvB`, `3.99 MWU` |
| Units | Muted, after a space. In tables the unit moves into the column header ("Value, BTC") and cells hold bare numbers, except sizes, which keep their unit in the cell because one column spans B to MB. A unit hidden from sight is still in the text for screen readers | |
| Approximate | `~` for projected values, `≈` for derived counts | `~0.64 sat/vB`, `≈ 15 blocks` |

### Spent state

An output is **Spent** (or **Being spent**, while the spending transaction is unconfirmed), **Unspent**, or **Unspendable**. Unspendable is for outputs that can never be spent, which the node reports as not spent: an `OP_RETURN` output, a locking script over the 10,000-byte limit, and the genesis block's coinbase, which was never added to the set of spendable coins. Never call those Unspent, and never call the genesis coinbase Spendable; the inspector gives the reason.

### Hashes and copy

- **In lists:** first eight and last eight characters with a middle ellipsis (`a3f9c2e1…7b0d44e9`), in the monospace cut. The full value is in the `title` and in the copy button.
- **As a page subject** (block hash, txid): the full value, broken into groups of eight that alternate between text and muted color, wrapping cleanly at any width. A copy button sits at the end.
- **Addresses** in the flow diagram: seven and five characters.
- **Copy button:** a 28 px icon button. On press it turns positive, pops once and its label reads "Copied" for 1.4 s, only when the value was copied. When the browser copied nothing, its icon turns to a cross in the negative color, "Not copied" shows above it and "Could not copy" is announced, for 3 s.

### Time

Relative time ("13 min ago") in lists, with the absolute UTC time in the `title`. Detail pages show absolute UTC first (`2026-11-11 20:41:07 UTC`) and relative time below. Under 10 seconds is "just now". Block tiles drop "ago" to fit ("13 min").

## Space, shape and layout

- **Space** on a 4 px grid: `--space-1` 4, `--space-2` 8, `--space-3` 12, `--space-4` 16, `--space-5` 24, `--space-6` 32, `--space-7` 48, `--space-8` 64.
- **Page:** max width `--page-max` (85rem), side gutter `--gutter` (16 px on phones, up to 40 px).
- **Radius follows hierarchy:** `--radius-s` 2 px for squares and swatches, `--radius-m` 6 px for controls and tiles, `--radius-l` 12 px for panels, `--radius-pill` for chips. Do not use one radius for everything.
- **Sections are open, not boxed.** A section is a heading over a 2 px rule in text color. Content sits on the page background. Only things that are objects (tiles, the grid frame, flow items, tooltips) get a surface.
- **Shadow** only on floating things: `--shadow-pop`.
- **Tiles:** `--tile-w` 8rem, `--tile-h` 9.75rem, `--tile-gap` 0.875rem. Phones override them to 7.5rem and 9.25rem on the strip element.
- **Breakpoints:** 1040 px (single column, search moves under the header) and 640 px (phone).

## Motion

| Token | Value | Use |
|---|---|---|
| `--dur-instant` | 90 ms | Press, toggle |
| `--dur-quick` | 160 ms | Hover, focus, tooltip |
| `--dur-base` | 260 ms | Open, expand, swap |
| `--dur-slow` | 520 ms | Tiles shifting, bars resizing, pins sliding |
| `--dur-block` | 900 ms | The new-block sequence, end to end |
| `--dur-fade` | 140 ms | Opacity fades that remain under reduced motion |
| `--stagger` | 45 ms | Delay between items in a sequence |
| `--ease-out` | `cubic-bezier(0.2, 0.7, 0.1, 1)` | Things arriving or moving into place |
| `--ease-in-out` | `cubic-bezier(0.65, 0, 0.35, 1)` | Things travelling from one place to another |
| `--ease-settle` | `cubic-bezier(0.3, 1.45, 0.6, 1)` | Things that land: a small overshoot |

Rules:

1. **Move with `transform`, fade with `opacity`.** Never animate width, height, top or left. Bars that change size are positioned with `translateX` and sized with `scaleX` (see the mempool bar and the fee ladder).
2. **Read durations from the tokens** in JavaScript too (`getComputedStyle(...).getPropertyValue("--dur-slow")`), so reduced motion applies everywhere.
3. **Leaders move first.** When a row of things shifts, stagger from the one at the front of the movement so nothing overlaps.
4. **One moment per event.** A new block is one sequence. Live rows get a background flash, not a slide.
5. **Reduced motion:** all durations except `--dur-fade` become 0. Nothing translates or scales. Content still updates, and short opacity fades remain. Ambient loops (the live dot) are opacity only.

### CSS or WebGL

| Scene | Drawn with | Why |
|---|---|---|
| Block strip (up to about 20 tiles) | DOM, CSS and the Web Animations API | Few elements; text must be selectable and accessible |
| Mempool bar, fee ladder, flow diagram, timeline | DOM and SVG | Few elements |
| Hover, focus, press, tooltips, row flash | CSS transitions | |
| A block's packed transaction grid (thousands of squares) | WebGL2 instanced quads | Too many elements for the DOM |
| The projected-block mempool view and squares moving from mempool into a block | WebGL2 | Thousands of moving squares |

The prototype draws the grid with canvas 2D as a stand-in. The real app uses WebGL ([`ARCHITECTURE.md`](ARCHITECTURE.md)).

### The new-block sequence

When a block is found, in 900 ms:

1. The "now" line flashes in the accent (opacity).
2. The next projected tile crosses the line on a shallow arc (lift 14 px, scale 1.05 at the midpoint), lands in the first mined slot with a short squash (`--ease-settle`), and changes from dashed to solid. Its border shows the accent until the sequence ends.
3. Mined tiles shift right by one slot, the farthest first. The last mined tile fades out.
4. Projected tiles shift toward the line, the nearest first, and relabel ("Next block", "In ~20 min").
5. A new projected tile fades in at the far end and its strata drop in one layer at a time, cheapest first, like sediment.
6. Mempool figures, the mempool bar and the fee ladder pins update and slide to their new values.
7. A polite live region announces "Block 975 412 mined by …: 3 341 transactions, median fee 0.64 sat/vB."

The sequence uses FLIP: measure every tile, change the DOM, then animate each tile from its old place to its new one with `transform`.

### The projected side

Decided while building it, where the rest of this document left it open:

- **Four tiles:** the next three blocks and one for everything after them, which names how many blocks' worth it holds ("+14 blocks"). They read "Next block", "In ~20 min", "In ~30 min"; regtest has no block interval, so there they read "Second block" and "Third block". The next block's foot says "~10 min". Each shows `~` before its median, its range and its transaction count.
- **Each projected tile opens its projected block's page;** the last tile opens the first block it holds. Its link carries the tile's meaning for a screen reader ("Next block, projected: 3 214 transactions, median fee about 1.50 sat/vB, …").
- **Ordinary updates do not move.** The mempool changes every two seconds; a tile's figures and layers change where they stand, with no transition, so the strip is simply different the next time you look. Motion is kept for tiles changing places: a block mined, a projection catching up after a burst or a reorg, a tile forming or leaving. A tile that appears outside a crossing (the first state, a mempool growing into a second block) fades in and settles its layers; one that leaves fades where it stood.
- **The crossing shows the projection becoming the block.** The tile that crosses is the one that projected that height, and it carries the projected figures (with `~`) and layers until it lands, however soon the block's own facts arrive. Then each real layer grows out of the same band's projected layer while the projected ones fade: a block close to its projection barely moves, a different one visibly re-forms, and an empty block drains to its coinbase. In a burst each block leaves from the tile that projected it.
- **A small mempool is information.** One short tile is the whole mempool, and the space beside it says "Everything waiting fits in the next block." An empty next block says "Nothing waiting" over a dashed floor, and the space beside it "Nothing is waiting. The next block would hold only its coinbase." (Phones hide that sentence; the tile still says it.)
- **Not live, honestly.** When the server has lost its node, the last projection stays, the space beside it says it is the last one before the server lost touch, and the live indicator says "Node not answering". When the server does not track the mempool, the strip keeps the single "Next block" tile with the time since the last block.
- **New transactions come in at a readable pace,** one row at a time, at most one every 0.7 s, newest on top with the row flash; on a busy day the rows that could not be shown in time are skipped rather than queued.

### The transaction grid

Decided while building it, where the rest of this document left it open:

- **It is a core sample too.** The frame is the weight limit, so a full block fills it and an old block of 20 kvB is a thin layer on the floor. The squares are packed from the top left in fee order, then each drops until it rests on the floor or on another square: the floor is flat, the surface is ragged, the dearest transactions sit on top and the cheapest at the bottom, as in the strata. Settling can sink a square a few rows into a hole below it, so "highest at the top left" is where the dearest start, not a strict rule.
- **Honest scale.** A square's area is its virtual size, its side rounded to whole cells (about 6.5 vB per cell for a block, so the median transaction is 4 or 5 cells across and its area is right to within about 25%). The smallest side is 2 cells: anything smaller is drawn at that size, its area overstated, so it can be seen and pointed at. No mainnet transaction is that small at a block's scale. A block holding only its coinbase is one small square on the floor; the caption gives its fullness in words.
- **The coinbase** is the first square, at the top left, in `--color-text-muted`: it pays no fee rate, and color carries only fee.
- **Pour in, cheapest first.** On load the squares drop into place from a little above, the cheapest first and the dearest last, each with `--ease-settle` over `--dur-slow`, the whole pour lasting `--dur-block`, like the strata settling. Under reduced motion they fade in where they lie over `--dur-fade`. Live changes: a square that moves travels with `--ease-in-out`, leaders first; a new one drops in; a gone one shrinks and fades where it stood.
- **Hover** rings the square in `--color-text` and opens a tooltip above it (below when there is no room), inside the frame: fee rate with its swatch, txid, fee, size and value; the coinbase says "Coinbase" and what it pays. Click opens the transaction. On touch the first tap shows the tooltip ("Tap again to open"), the second opens it.
- **No keyboard stops in the grid.** Thousands of squares in a packing have no order an arrow key could follow, and a screen reader cannot see them. The list of transactions below is the keyboard and screen-reader path to the same facts and links; focusing or hovering a row rings its square, and the caption links to the list.
- **Captions.** The fee-layer caption and the grid's sit in the same cell, so the taller sets the height and swapping one for the other moves nothing.
- **Fallbacks.** The fee layers stay in the frame until the grid has drawn, and come back if the WebGL context is lost, until it is restored. A browser without WebGL2 keeps the layers and the caption says the map needs WebGL2. If the data fails, the caption offers "Try again".

### The projected-block page

Decided while building it (`/block/next`, `/block/next/:n`), where the rest of this document left it open:

- **The address is a place in the queue,** not a height. `/block/next` is whatever block comes next, so the link keeps meaning "the next block" after one is mined, and the block that was mined gets its own `/block/:hash`. Positions run from 1 (the next block) to 8 (the rest of the mempool), as the projected tiles do.
- **Provisional, in the same frame.** The map sits in the box a mined block's map gets, with the same scale (the frame is the weight limit), but the frame is dashed and open, the level line is dashed, and the title carries a dashed "Projected" badge. Figures that are estimates carry `~` (median rate, range, fullness); counts and fees of the projection itself are exact.
- **Motion for live changes is the grid's own** and nothing more: a square that moves travels with `--ease-in-out` over `--dur-slow`, leaders first; a newcomer drops in with `--ease-settle`; one that is outbid, replaced or mined shrinks and fades where it stood over `--dur-base`. An update's motion is over in about 0.7 s of each 2 s interval, so the picture is still more than half the time and a change reads as one event. Squares whose place did not change do not move. Nothing else on the page animates per update: rows that came in get the row flash once, the "last change" line says how many came in and left, and no live region reads updates out.
- **A block found is the one orchestrated moment.** The dashed frame sets solid in the accent for `--dur-block` while the mined squares fade and what is left moves up; a notice in the accent says which block was found, when, and links to it, and a polite live region says it once. On a page with a map the moment waits for the block's new picture: on mainnet the projection can sit on the new tip one update before the mined transactions leave it, so telling it at the tip alone would show the mined block under a notice that says it has moved on (seen live at block 975 529). It waits at most 8 s. The next block's page then carries on with the new next block. A later block's page stays at its place in the queue, since its address names the place, and the notice says that every projected block moved up, linking to where what it showed went (or saying it was the guess at one of the blocks found, in a burst deeper than it). Blocks found in a burst are one notice ("Blocks 101 to 103 were found"); a shorter or equal chain with a new tip is a reorg and says so. The notice stays until dismissed.
- **Empty is a state, not a failure.** On test networks the next block is often empty: the frame stays, and its open middle says "Nothing waiting here" and what that means, fading in after the last squares have gone.
- **The rest of the mempool has no map.** It has no size bound, so it gets its strata (squeezed into one frame when it holds more than a block, and labelled so) and a sentence on why there is no map.
- **Not live, not current, not on.** The connection state sits beside the badge as an icon and a word. While the server has lost its node the page keeps its last picture with a caution notice saying since when. With the mempool switched off on a network, the page says so and points to the mined blocks.

### The transaction page, live

Decided while building it, where the rest of this document left it open. The page is where people land from a wallet, often worried, so it says exactly what the server knows and no more.

- **Five words for where it is.** Unconfirmed (in the mempool), Confirmed, Replaced (another transaction spending the same coins took its place), Dropped (it left without being mined, for a reason the node does not report) and Removed (the node no longer has it and nothing says why). The timeline's middle step carries the same word with its detail; a replaced version's last step is "Will not confirm", struck through.
- **Where it waits** sits above the fee, beside the flow, so the answer is in the first screen. The projected blocks are a row of dashed slots running to a dashed "now" line, the next block nearest it, the last one "8+". The transaction is one square in the fee colour of the group it will be mined with. Under it, in words: "In the next block" (linked to that block's page), "In about 3 blocks", or "In more than 7 blocks"; how much would be mined before it; and what that means in time. A block is on average ten minutes away whenever one looks, and about one in seven takes more than twenty (an exponential wait), so the next block is said that way; later blocks get a rough figure ("~30 min") at the usual pace, with the condition that nothing paying more arrives; regtest says it has no schedule. A last, muted line says this is a projection, not a promise.
- **Alive without twitching.** The square travels only when its block changes (`--dur-slow`, `--ease-in-out`); what is ahead of it changes in place. Nothing is announced for a position that only moves: only entering or leaving the next block, and every change of state, once, in a polite live region.
- **The position bar of the prototype is not used.** The projection says which block a transaction is in, not a percentile of one, and the lane says that directly.
- **Mined with.** When the rate it will be mined at prints differently from its own, an "Effective fee rate" fact joins the fee list, and under it the group: its members in mining order (this transaction, its child, its parent), each with its own rate and a link, one sentence saying who lifts whom ("On its own it pays 1.00 sat/vB, but its child pays enough to lift the group to 12.5 sat/vB as a whole") and one explaining why miners take such a group whole, naming child pays for parent last.
- **Replaced** is a notice above the timeline, in the negative colour: what took its place, linked, with both rates and both fees, and that this version can no longer confirm. A longer chain is listed oldest first, and the newest version's own state ("It waits in the mempool: in the next block", "It confirmed in block 975 530") is followed live. Opened later, when the node says not found, the same notice comes from the server's record of replacements; when the record has nothing, the not-found state says the record is recent and holds nothing for it, and that the page will show the transaction if it is broadcast while open.
- **Dropped** is a notice that lists what it may have been (pushed out by fees, expired, made invalid by a reorganisation, or replaced by a transaction the server did not see) and that it will not confirm unless broadcast again. It never says "replaced".
- **The happy moment** is the existing one: the "Confirmed" step's marker lands with `--ease-settle` over `--dur-slow`, once, and the live region says "Confirmed in block N." Reduced motion keeps only the change.
- **Back from a reorganisation** is a caution notice ("Back in the mempool") naming the block that held it, saying it waits to be mined again and can be replaced meanwhile, until it confirms again.
- **Not live, quietly.** Beside the badge, an icon and a word: "Live" (a breathing dot), "Connecting", or "Not live" / "Node not answering" with what the page does instead ("Checking every 10 s instead."). Without the live channel there is no position to vouch for, so the panel says the place in the queue shows while the page is live; with the mempool engine off on the network the panel is left out. A settled transaction shows no indicator: nothing on it can change.
- **The address page** shows "Live" under its header while the node watches the address, and otherwise says plainly that the page does not update by itself and why (too much history to watch, the node watching as many addresses as it can, the node out of reach), never as an error.

## Components

| Component | What it is |
|---|---|
| Header | Logo and wordmark, network switch, search, navigation (Blocks, Mining, Lightning), theme toggle. Sticky. On phones: logo, network and theme on row one, search on row two, navigation on row three |
| Network band | Hatched band in the network color with a plain-language warning. Test networks only |
| Logo | `design/logo.svg`: a tile with three strata, the top layer in the accent and offset, still settling. In the header it is inline SVG with the tile in `--color-network`. The wordmark is live text in the heavy weight, so a new name is one string |
| Block tile | Projected (dashed) or mined (solid). Height or label, median fee, fee range, strata, transaction count and age. Mined tiles link to the block and show the pool below |
| Block strip | Projected lane, "now" line, mined lane. Scrolls sideways inside itself on narrow screens and starts centered on the line |
| Strata | Fee bands stacked by virtual size, inside a tile. Fill height is how full the block is. A band with any transactions is at least about 2 px thick where the block's height leaves room; in a nearly empty block the layers share its true height and the smallest may not show. Until the block's fee data arrives, a neutral gauge of the same height stands in |
| Mempool bar | Horizontal strata of the whole mempool. Each band has a tooltip and is focusable |
| Fee key | Ten swatches with lower bounds |
| Fee ladder | A log scale from 0.1 to 20 sat/vB with pins for the four recommendations. Pin labels stack into rows so close values never collide, and flip near the right end. A list below gives each priority with its expected wait |
| Fee chip | Swatch and number, with or without the unit |
| Amount | BTC with dimmed trailing zeros. sats on hover |
| Hash | Truncated or grouped, monospace, with copy |
| Stat | Label above a large figure, optional muted sub-line |
| Fact list | `dl` of label and value pairs, two columns or one |
| Table | Units in headers, bare numbers in cells, right-aligned numeric columns, row rules. Phones drop the least important column |
| Packed grid | Squares sized by vsize and colored by fee band, highest fee rate at top left, resting on the floor of the frame ([The transaction grid](#the-transaction-grid)). Tooltip on hover, click opens the transaction, rows in the table below highlight their square. Comes with a caption and the fee key; the table is its text alternative |
| Flow diagram | Inputs on the left, outputs on the right, ribbons as thick as the value they carry, meeting at a bar that is the transaction. The fee leaves as its own ribbon in its fee color |
| Status badge | Pill with icon and word: Confirmed (check), Unconfirmed (dashed circle), Replaced, Dropped and Removed (cross) |
| Timeline | Numbered steps: first seen, replaced or replacing, waiting or confirmed. Horizontal on wide screens, vertical on phones. A version that can no longer confirm is struck through with a dashed marker |
| Notice | Bordered strip for important state, such as "This version was replaced" with a link to the replacement |
| Fee position | The comparison block's strata laid on their side with a pin where this transaction's fee rate sits |
| Tooltip | Raised surface, title line and muted detail lines. Opens on hover and keyboard focus |
| Buttons | Default (outlined) and accent (filled). Toggles use `aria-pressed` and show a dot when on |

## Page layouts

### Home

```text
┌ header ─────────────────────────────────────────────────────────────┐
├ strip band (sunken, full width) ────────────────────────────────────┤
│ Blocks   ┆ projected ▭ mined ■                 [Mine a block][Auto] │
│ [+11][~30m][~20m][Next] ┆now┆ [975411][975410][975409][975408] …   │
├─────────────────────────────────────────────────────────────────────┤
│ Mempool ═══════════════════════════  │ Fee to pay now ═════════════ │
│ Transactions  Size  Fees  Lowest     │   pins on a log scale        │
│ [██████ mempool bar ████▌▌▌]          │ ●────●──●────●───────────    │
│ fee key                              │ No rush  Low  Medium  High   │
├──────────────────────────────────────┼──────────────────────────────┤
│ Latest transactions ════════════════ │ Difficulty adjustment ══════ │
│ txid     fee rate    size    value   │ [████████████████░░┃░░]      │
│                                      │ 1 797 of 2 016 blocks        │
│                                      │ Expected  Next  Previous     │
└──────────────────────────────────────┴──────────────────────────────┘
```

The difficulty bar is neutral (never a fee color); its mark is where the period would be at one block every ten minutes. Changes read as an arrow and a word ("Down 7.05%"). On a network that never retargets (regtest) the section says so instead.

Phones: one column; the strip scrolls sideways; stats in two columns.

### Block

```text
‹ Previous: 975 410                                   Next: 975 412 ›
Block 975 411
00000000 00000000 0004f2c1 …  [copy]
┌ packed grid (3/5) ─────────────┐  ┌ facts (2/5) ────────────────┐
│ ▇▇▆▆▅▅▄▄▄▃▃▃▂▂▂▁▁▁             │  │ Mined          Miner        │
│                                │  │ Transactions   Size         │
└────────────────────────────────┘  │ Median fee     Fee range    │
caption, fee key                     │ Total fees     Reward       │
                                     └─────────────────────────────┘
Transactions ═══════════════════════════════════  25 of 3 342, by fee rate
txid  fee rate  fee  size  value                         [Show 25 more]
```

### Transaction

```text
Transaction  (✓ Confirmed)
65428488 a6202f1d 681d053b …  [copy]
[notice: this version was replaced → replacement txid]   (replaced only)
① First seen ──── ② Replaced an earlier version ──── ③ Confirmed
┌ Inputs and outputs (3/5) ─────────────────┐ ┌ Fee (2/5) ───────────┐
│ [input]  ═══╗          ╔═══ [output]      │ │ Fee rate, fee, vsize │
│ [input]  ═══╣ ┃trunk┃  ╠═══ [output]      │ │ chunk fee rate       │
│             ╚══════════╚~~~ [fee]         │ │ fee position bar     │
└───────────────────────────────────────────┘ └──────────────────────┘
```

Long input or output lists collapse after ten; each "Show more" adds a hundred, and the hidden rest travels as one ribbon, so a transaction with thousands of outputs never renders them all at once.

### Address (not in the prototype)

```text
Address
bc1qxy2k gdygjrsq tzq2n0yr f2493p83 kkfjhx0w lh   [copy]  [QR]
Balance (big)   Received   Sent   Transactions   Unconfirmed
[balance-over-time line chart]
Transactions ═════════════════════════════════════════════════════
time  txid  this address +/-  (positive or text color)  fee rate  status
```

Incoming amounts are `+` in positive color with the sign, outgoing `−` in text color. Color never carries the direction alone.

### Mining (not in the prototype)

```text
Mining                                        [24 h | 1 w | 1 m | 1 y | All]
(network note: signet, testnet4, regtest)
The network now ═══════════════════════════════════════════════════════════
Hashrate   Difficulty        │ Next difficulty adjustment (the home panel's)
Hashrate and difficulty ═══════════════════════════════════════════════════
[area: hashrate; dashed step: difficulty, right axis]
Pools ═════════════════════════════════════════════════ 895 blocks in the last week
rank  pool  share bar  blocks  hashrate  empty │ Share of blocks, week by week
...                                           │ [stacked strata, biggest at the bottom]
Rewards ═══════════════════════════════════════════════════════════════════
Paid to miners   New coins   Fees   Per block
Blocks over time ══════════════════════════════════════════════════════════
[Fees | Rewards | Fee rates | Size]  one chart at a time
Recent blocks ═════════════════════════════════════════════════ All blocks
the same mined tiles as the strip, in a wrapping grid
```

Decided while building it, where the layout above left it open:

- **The period is in the address** (`/mining?period=1m`), so a view can be linked; a week is the default. "All" joins the design's four periods, for the whole of history. The period is a row of links, the current one raised with an accent underline.
- **Charts are drawn with the text color, never fee colors,** since color carries fee and nothing else: the main series a solid line over a pale area, a second series a dashed line on its own right-hand scale. The one exception is the fee-rate chart, whose axis carries the fee scale as its key.
- **Pool shares over time are strata.** Each pool is a layer, the biggest at the bottom, in alternating shades of the text color with a seam of the page between them: the same core sample as the tiles, laid on its side through time. The seven biggest pools have their own layer and the rest are one, "Other pools". The layer under the pointer turns the accent. Shares are weekly, so the chart appears from a month up; shorter periods say so.
- **Hashrate needs a week.** Each daily point averages the 7 days before it, so the 24-hour view draws the last week and says why.
- **Coverage is drawn, not hidden.** Where the server has not stored the start of the period yet, the chart keeps the whole period's width and hatches the part with no data ("No data stored"), and a note above says "Data from 22 Sep 2026 onward". The same hatch is the note's swatch. A period with nothing stored says so and offers all of history.
- **One readout, no tooltip.** Above each plot a line at a fixed height shows the point under the pointer (the latest otherwise): its time and every figure with its key. It never covers the data, works the same for touch and keys, and never moves the page.
- **The second level is a switch,** not more charts: fees, rewards, fee rates and size share one chart slot under "Blocks over time", so the page stays one screen of figures and one of charts.
- **Unknown is a pool.** It is ranked like any other, its page exists, and its row says it is the blocks no tag or address on the list names.
- **Test networks say what their figures mean.** Signet's blocks come from a few signers the list does not name; testnet4's minimum-difficulty rule makes its hashrate swing; regtest has no pools, a meaningless hashrate and no retargets. A note under the title says so, and figures that mean nothing there say "Not meaningful on regtest".
- **A pool's page** has its name as the title (dataset text), its site as a plain link only when it is `http` or `https`, its blocks and shares, its weekly share and hashrate, and its blocks 25 at a time.
- **Nothing moves as it loads.** Each section holds the height its content will take; the coverage notes on a pool's page sit below its figures.

### Mempool history (not in the prototype)

```text
Mempool history                     [2 h | 24 h | 1 w | … | 4 y | All]
(coverage sentence, with the hatch as its swatch, when minutes are missing)
Waiting, by fee rate ════════════════════════════════ One point a minute
readout: time, total, transactions, lowest accepted, each band
[stacked areas in the fee colors, cheapest at the bottom; total as a line]
fee key
Flow ═══════════════════════════════════════════════════ vB a second
Over the last 24 hours, 2 983 vB/s arrived, blocks took …: it filled by …
[line: arrived; stacked shades: mined, left otherwise]
```

- **The mempool is the core sample laid on its side,** as pool shares are, but its layers are fee bands, so here they take the fee colors: color still carries fee and nothing else. The layer under the pointer turns the accent.
- **Flow is its own chart,** not a second axis on the first: its units are a rate, and the question it answers (filling or draining) is read from where the line sits against the layers. A sentence above gives the period's answer in words.
- **Honest about coverage,** as the mining charts are: the hatch before the first record, a break wherever a step was not sampled (a lone minute is a sliver), and a sentence saying how many of the period's minutes are here and since when.
- **Eleven intervals** wrap into two even rows on a phone.

### Lightning (not in the prototype)

```text
Lightning                                   Find a node or channel [ … ][Search]
The network now ═════════════════════════════════════════ As of 12 Nov, 18:30 UTC
Channels  Nodes  Capacity  Median fee rate  Median base fee
(only channels checked against the chain)
Over time ═══════════════════════════════════════════════════ Every ten minutes
[24 h | 1 w | 1 m | 3 m | 1 y | All]   (since when, with the hatch)
[area: channels; dashed line: capacity, right axis]
Newest channels ══════════════════ │ Newest closes ═════════════════════
channel and ends, capacity, block  │ channel, kind and ends, capacity, block
Largest nodes ════════════════════ │ Most connected nodes ══════════════
Node page: name (text, cut), key grouped with copy, "the key is the identity";
  figures; addresses and features as chips; channels, open | closed, 25 a page
Channel page: id and state badge; funding; close (kind · how sure, why); its two ends' policies
```

Decided while building it:

- **What is ours is said, not hidden.** Under the figures, one line says only channels checked against the chain are counted, so the counts run lower than sites that list what gossip claims. History starts when this server's peer first synced: the chart hatches before that and says since when, and the closed lists say they hold closes seen since then.
- **A close says how sure it is,** beside its kind: "Force close · Certain", "Cooperative close · Likely" (a splice that only takes funds out looks the same), "Penalty · Certain", "Other · Certain it is not a plain close". Lists give the kind in one word; the channel page gives the reason.
- **Charts are drawn with the text color,** as the mining charts are: the main series an area, the second a dashed line on its own scale.
- **Names are claims.** A node's name is text, cut short in lists and titles with the rest one press away, and isolated so its direction cannot run into the text beside it; its public key is always next to it. Node colors are not shown: color carries fee and nothing else, and an operator's chosen color would only add a second way to imitate another node.
- **Tables fit a 320 px phone.** A channel row puts its id, kind, time and both ends in one cell; a node list drops, on a phone, the figure it is not ranked by.
- **Switched off is a sentence,** not an empty chart: a network without Lightning says so, and a peer still syncing shows how far it has come and fills the page when done.

## Plain CSS or a utility framework

**Decision: plain CSS with custom properties.** The lean holds.

1. **The tokens are already the system.** `tokens.css` is plain custom properties. A utility framework would need the same tokens restated in its config and would add a build plugin to keep them in sync.
2. **No dependency.** Vite bundles plain CSS natively. Nothing to audit, upgrade or configure.
3. **Most styling here is not utility-shaped.** The hard parts are the strip, the grid, the flow and the ladder: positioned, measured and animated components that are custom CSS in either approach.
4. **Solid markup stays readable.** Class names describe the component (`tile__strata`) rather than its look.
5. **It meets the CSP.** Static CSS files only. Dynamic values go through `element.style.setProperty`, which `default-src 'self'` allows, never through `style` attributes in markup.

How to write it:

- Import `tokens.css` once, unchanged. It contains only `@font-face` and custom properties.
- Order everything else with cascade layers: `@layer reset, base, layout, components, pages;` so specificity never fights. `tokens.css` stays unlayered.
- One CSS file per component next to the component, with BEM-style names (`block`, `block__part`, `block--variant`).
- Pass per-element values (a fee color, a position) as custom properties set from code, such as `--c`, `--x` and `--w`.
- Override a token for a region by setting it on that region's element, not on `:root`. Unlayered `:root` rules in `tokens.css` beat layered ones.

## Prototype

`design/prototype/` holds `index.html` (home), `block.html` and `tx.html`, with plain ES modules and no build step. It needs an HTTP server, because browsers do not load fonts or modules from `file://`. From `design/`, run any static server on a free port, for example `python3 -m http.server 0`, then open `/prototype/`.

- Each page carries `<meta http-equiv="Content-Security-Policy" content="default-src 'self'">`, so anything the real policy would block shows up as a console error.
- URL parameters: `?network=signet|testnet4`, `?theme=light|dark`, `?autoplay=0` on the home page, `?h=<height>` on the block page, `?state=unconfirmed|replaced|confirmed` on the transaction page.
- The home page mines a block every 6.5 seconds. "Mine a block" triggers it at once, and "Auto-play" toggles the loop.
- The sample data is invented, seeded and repeatable. It follows late-2026 fee levels: most transactions pay under 1 sat/vB, and the floor is 0.1.
