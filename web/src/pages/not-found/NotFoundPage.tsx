import { Link } from "../../components/Link";
import { SearchField } from "../../components/SearchField";
import { paths } from "../../lib/network";
import { usePageTitle } from "../../lib/page";
import "./NotFoundPage.css";

export default function NotFoundPage() {
  usePageTitle(() => "Page not found");
  return (
    <div class="wrap not-found">
      <h1>Page not found</h1>
      <p class="not-found__text">
        Nothing lives at this address. Search for a transaction, address or block, or go on to one
        of the pages below.
      </p>
      <div class="not-found__search">
        <SearchField id="not-found-search" />
      </div>
      <nav class="not-found__links" aria-label="Where to go">
        <Link href="/">Latest blocks and fees</Link>
        <Link href={paths.blocks()}>Every block</Link>
        <Link href={paths.mempool()}>Mempool history</Link>
        <Link href={paths.mining()}>Mining</Link>
        <Link href={paths.lightning()}>Lightning</Link>
      </nav>
    </div>
  );
}
