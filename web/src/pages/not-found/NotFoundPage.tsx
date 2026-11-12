import { Link } from "../../components/Link";
import { usePageTitle } from "../../lib/page";
import "./NotFoundPage.css";

export default function NotFoundPage() {
  usePageTitle(() => "Page not found");
  return (
    <div class="wrap not-found">
      <h1>Page not found</h1>
      <p class="not-found__text">
        Nothing lives at this address. Search for a transaction, address or block above, or go to
        the <Link href="/">latest blocks</Link>.
      </p>
    </div>
  );
}
