# Design

The design language for mempool.based. The tokens live in [`design/tokens.css`](../design/tokens.css). A static prototype of the home, block and transaction pages is in [`design/prototype/`](../design/prototype/).

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

- **One file, 38 KB:** `design/fonts/recursive.woff2`, a variable font with `wght` 300–800 and `MONO` 0–1. Casual, slant and cursive axes are pinned off, and the character set is Basic Latin plus the punctuation we use (–, —, …, ≈, ≥, arrows, ₿, thin and narrow no-break spaces).
- If the font fails, the stack falls back to the system UI font and system monospace.
- **Turn on monospace** with `font-family: var(--font-mono); font-variation-settings: var(--mono-on);`. Use it for hashes, txids, addresses and scripts only. Numbers stay proportional with tabular digits.
- Rebuild the subset from the upstream release with fontTools:

```sh
fonttools varLib.instancer Recursive_VF_1.085.ttf CASL=0 slnt=0 CRSV=0 wght=300:800 -o rec.ttf
pyftsubset rec.ttf --flavor=woff2 --output-file=recursive.woff2 \
  --unicodes="U+0020-007E,U+00A0,U+00B0,U+00B7,U+00D7,U+2009,U+2013,U+2014,U+2018,U+2019,U+201C,U+201D,U+2026,U+2190-2193,U+2197,U+2212,U+2248,U+2264,U+2265,U+202F,U+20BF" \
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
| Size | vB under 10 k, then kvB and MvB with two decimals | `209 vB`, `14.62 MvB` |
| Units | Muted, after a space. In tables the unit moves into the column header ("Value, BTC") and cells hold bare numbers | |
| Approximate | `~` for projected values, `≈` for derived counts | `~0.64 sat/vB`, `≈ 15 blocks` |

### Hashes and copy

- **In lists:** first eight and last eight characters with a middle ellipsis (`a3f9c2e1…7b0d44e9`), in the monospace cut. The full value is in the `title` and in the copy button.
- **As a page subject** (block hash, txid): the full value, broken into groups of eight that alternate between text and muted color, wrapping cleanly at any width. A copy button sits at the end.
- **Addresses** in the flow diagram: seven and five characters.
- **Copy button:** a 28 px icon button. On press it turns positive, pops once and its label reads "Copied" for 1.4 s. If the clipboard is blocked, the press still shows its state.

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
- **A block found is the one orchestrated moment.** The dashed frame sets solid in the accent for `--dur-block` while the mined squares fade and what is left moves up; a notice in the accent says which block was found, when, and links to it, and a polite live region says it once. The next block's page then carries on with the new next block. A later block's page stays at its place in the queue, since its address names the place, and the notice says that every projected block moved up, linking to where what it showed went (or saying it was the guess at one of the blocks found, in a burst deeper than it). Blocks found in a burst are one notice ("Blocks 101 to 103 were found"); a shorter or equal chain with a new tip is a reorg and says so. The notice stays until dismissed.
- **Empty is a state, not a failure.** On test networks the next block is often empty: the frame stays, and its open middle says "Nothing waiting here" and what that means, fading in after the last squares have gone.
- **The rest of the mempool has no map.** It has no size bound, so it gets its strata (squeezed into one frame when it holds more than a block, and labelled so) and a sentence on why there is no map.
- **Not live, not current, not on.** The connection state sits beside the badge as an icon and a word. While the server has lost its node the page keeps its last picture with a caution notice saying since when. With the mempool switched off on a network, the page says so and points to the mined blocks.

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
| Status badge | Pill with icon and word: Confirmed (check), Unconfirmed (dashed circle), Replaced (cross) |
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
[balance-over-time line chart, uPlot]
Transactions ═════════════════════════════════════════════════════
time  txid  this address +/-  (positive or text color)  fee rate  status
```

Incoming amounts are `+` in positive color with the sign, outgoing `−` in text color. Color never carries the direction alone.

### Mining (not in the prototype)

```text
Mining ═══════ [24 h | 1 w | 1 m | 1 y]
Hashrate  Difficulty  Next adjustment (progress bar, estimate)
[hashrate and difficulty chart]
Pools: ranked list with share bars; the share bar is a neutral fill, not a fee color
Recent blocks: the same mined tiles as the strip, in a wrapping grid
```

### Lightning (not in the prototype)

```text
Lightning ═════
Nodes  Channels  Capacity  Median fee rate (ppm)
[capacity over time chart]
Top nodes by capacity / by channels: table
Node page: alias (text only), pubkey grouped with copy, color swatch, channels table
```

Node aliases and colors come from gossip and are untrusted: the alias is text, and the node color is shown only as a swatch with a border, never as text or background behind text.

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
