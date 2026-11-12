// Networks and the paths that carry them. Mainnet has no prefix; the others live under /<name>.

export const NETWORKS = ["mainnet", "signet", "testnet4", "regtest"] as const;
export type Network = (typeof NETWORKS)[number];

export const NETWORK_NAMES: Record<Network, string> = {
  mainnet: "Mainnet",
  signet: "Signet",
  testnet4: "Testnet4",
  regtest: "Regtest",
};

export function isNetwork(value: unknown): value is Network {
  return typeof value === "string" && (NETWORKS as readonly string[]).includes(value);
}

/** The network a page path belongs to. */
export function networkFromPath(pathname: string): Network {
  const first = pathname.split("/")[1];
  return first !== "mainnet" && isNetwork(first) ? first : "mainnet";
}

/** A page path without its network prefix: `/signet/tx/ab` → `/tx/ab`, `/signet` → `/`. */
export function stripNetwork(pathname: string): string {
  const network = networkFromPath(pathname);
  if (network === "mainnet") return pathname || "/";
  return pathname.slice(network.length + 1) || "/";
}

/** A network-free path (starting with `/`) placed on a network: (`signet`, `/tx/ab`) → `/signet/tx/ab`. */
export function onNetwork(network: Network, path: string): string {
  if (network === "mainnet") return path;
  return path === "/" ? `/${network}` : `/${network}${path}`;
}

/** Base URL of a network's public Esplora API. */
export function apiBase(network: Network): string {
  return `${onNetwork(network, "")}/api`;
}

const segment = (value: string | number) => encodeURIComponent(String(value));

/**
 * Network-free paths to each page. Chain data only ever lands in a path segment, encoded, so a
 * hostile value cannot change the scheme or the route (SECURITY.md rule 14).
 */
export const paths = {
  home: () => "/",
  block: (hashOrHeight: string | number) => `/block/${segment(hashOrHeight)}`,
  /** Recent blocks, newest first; from a height when paging back. */
  blocks: (from?: number) => (from === undefined ? "/blocks" : `/blocks/${segment(from)}`),
  /** A projected block by its place in the queue, 1 for the next; the last (8) is the rest. */
  projected: (position: number) =>
    position <= 1 ? "/block/next" : `/block/next/${segment(position)}`,
  tx: (txid: string) => `/tx/${segment(txid)}`,
  address: (address: string) => `/address/${segment(address)}`,
  mining: () => "/mining",
  lightning: () => "/lightning",
};
