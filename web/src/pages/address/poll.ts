// How the page keeps an above-cap address's unconfirmed transactions current. Its own file, so
// the browser tests can import the numbers they wait on.

/**
 * How often the unconfirmed list is asked again while the page is in view. The node refuses to
 * watch these addresses, so there is no live signal; each ask is the node's sub-millisecond
 * mempool lookup for one script (docs/API.md, "Very large addresses").
 */
export const POLL_MS = 10_000;
/** The node lists at most this many unconfirmed transactions for one address. */
export const MEMPOOL_ROWS = 50;
