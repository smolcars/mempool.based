import { Show } from "solid-js";
import { groupDigits } from "../lib/format";
import "./Button.css";
import "./Pagination.css";

export interface PaginationProps {
  /** Current page, from 1. */
  page: number;
  /** Total pages, when known. Cursor-paged lists (Esplora's) leave it out and pass `hasNext`. */
  pageCount?: number;
  /** Defaults to `page < pageCount`. */
  hasNext?: boolean;
  onPage: (page: number) => void;
  /** Names the list for screen readers: "Transaction pages". */
  label: string;
}

/** Previous and next with the current page between them. */
export function Pagination(props: PaginationProps) {
  const hasNext = () =>
    props.hasNext ?? (props.pageCount !== undefined && props.page < props.pageCount);
  return (
    <nav class="pagination" aria-label={props.label}>
      <button
        class="btn"
        type="button"
        disabled={props.page <= 1}
        onClick={() => props.onPage(props.page - 1)}
      >
        Previous
      </button>
      {/* Only the page is announced when it changes: a count that moves with a live list would be
          read out on every update. */}
      <span class="pagination__status num">
        <span aria-live="polite">Page {groupDigits(props.page)}</span>
        <Show when={props.pageCount}>{(n) => <>{` of ${groupDigits(n())}`}</>}</Show>
      </span>
      <button
        class="btn"
        type="button"
        disabled={!hasNext()}
        onClick={() => props.onPage(props.page + 1)}
      >
        Next
      </button>
    </nav>
  );
}
