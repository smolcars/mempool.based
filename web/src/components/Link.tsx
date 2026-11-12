import { type JSX, splitProps } from "solid-js";
import { onNetwork } from "../lib/network";
import { useNetwork } from "../lib/page";

export type LinkProps = Omit<JSX.AnchorHTMLAttributes<HTMLAnchorElement>, "href"> & {
  /** A network-free path from `paths` in lib/network.ts, such as `paths.tx(txid)`. */
  href: string;
};

/** An internal link that stays on the current network. The router handles the click. */
export function Link(props: LinkProps) {
  const [own, rest] = splitProps(props, ["href"]);
  const network = useNetwork();
  return <a {...rest} href={onNetwork(network(), own.href)} />;
}
