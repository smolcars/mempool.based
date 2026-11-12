# Design

> **Status: brief.** Task D1 ([`TODO.md`](TODO.md)) replaces this page with the design language and a static prototype under `design/`.

## Goal

A block explorer that is faster and better looking than mempool.space, with motion that makes live data easy to follow.

## What to keep from the genre

The ideas people already understand: blocks as tiles in a strip, with upcoming (projected) blocks on one side and mined blocks on the other, and a block's transactions drawn as a packed grid of squares sized by weight or value.

## What must be our own

The visual identity: logo, color, type, spacing, motion. Nothing is copied from mempool.space.

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

## Deliverables of D1

1. This page rewritten as the design language: principles, color tokens for both themes, type scale, spacing, motion rules, a component inventory, and layouts for the home, block, transaction, address, mining and Lightning pages.
2. A static prototype in `design/prototype/` (plain HTML, CSS and JavaScript, no build step) showing the home, block and transaction pages with sample data and the key animations.
3. A decision, with reasons, on plain CSS versus a utility framework.
