import { omit } from "solid-js";
import type { JSX } from "@solidjs/web";
import { onNetwork } from "../lib/network";
import { useNetwork } from "../lib/page";

export type LinkProps = Omit<JSX.AnchorHTMLAttributes<HTMLAnchorElement>, "href"> & {
  /** A network-free path from `paths` in lib/network.ts, such as `paths.tx(txid)`. */
  href: string;
};

/** An internal link that stays on the current network. The router handles the click. */
export function Link(props: LinkProps) {
  const rest = omit(props, "href");
  const network = useNetwork();
  // oxlint-disable-next-line jsx-a11y/anchor-has-content -- the content arrives in `rest.children`
  return <a {...rest} href={onNetwork(network(), props.href)} />;
}
