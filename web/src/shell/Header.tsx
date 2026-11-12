import { useIsRouting, useLocation } from "@solidjs/router";
import { type Accessor, onSettled } from "solid-js";
import { Link } from "../components/Link";
import { SearchField } from "../components/SearchField";
import { type Network, stripNetwork } from "../lib/network";
import { Logo } from "./Logo";
import { NetworkSwitch } from "./NetworkSwitch";
import { ThemeToggle } from "./ThemeToggle";
import "./Header.css";

const NAV = [
  { href: "/blocks", label: "Blocks", match: (p: string) => /^\/blocks?(\/|$)/.test(p) },
  { href: "/mining", label: "Mining", match: (p: string) => /^\/mining(\/|$)/.test(p) },
  { href: "/lightning", label: "Lightning", match: (p: string) => /^\/lightning(\/|$)/.test(p) },
];

export function Header(props: { networks: Accessor<readonly Network[] | undefined> }) {
  const location = useLocation();
  const isRouting = useIsRouting();
  const page = () => stripNetwork(location.pathname);
  let search: HTMLInputElement | undefined;

  // "/" jumps to search from anywhere but a text field.
  onSettled(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "/" || e.metaKey || e.ctrlKey || e.altKey) return;
      const t = e.target;
      if (
        t instanceof HTMLElement &&
        (t.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(t.tagName))
      )
        return;
      e.preventDefault();
      search?.focus();
      search?.select();
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  });

  return (
    <header class="site-header">
      <div class="site-header__inner">
        <Link class="brand" href="/">
          <Logo />
          <span class="brand__word">mempool.based</span>
        </Link>
        <NetworkSwitch networks={props.networks} />
        <div class="site-header__search">
          <SearchField ref={(el) => (search = el)} />
        </div>
        <nav class="nav" aria-label="Main">
          {NAV.map((item) => (
            // oxlint-disable-next-line unicorn/prefer-regexp-test -- `match` is our own function, not String#match
            <Link href={item.href} aria-current={item.match(page()) ? "page" : undefined}>
              {item.label}
            </Link>
          ))}
        </nav>
        <ThemeToggle />
      </div>
      <div
        class={["route-progress", { "route-progress--on": isRouting() }]}

        aria-hidden="true"
      />
    </header>
  );
}
