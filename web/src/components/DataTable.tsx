import { For, Show } from "solid-js";
import type { JSX } from "@solidjs/web";
import { Link } from "./Link";
import "./DataTable.css";

export interface Column<T> {
  /** Header text. Units go here ("Value, BTC"), so cells hold bare numbers. */
  header: string;
  cell: (row: T, index: number) => JSX.Element;
  /** Right-aligned numeric column. */
  numeric?: boolean;
  /** Dropped on phones. Mark the least important columns. */
  optional?: boolean;
  /** A shorter header shown on phones ("Txs"); screen readers keep the full one. */
  short?: string;
}

export interface DataTableProps<T> {
  /** Says what the table lists; read by screen readers, hidden unless `showCaption`. */
  caption: string;
  showCaption?: boolean;
  columns: Column<T>[];
  rows: readonly T[];
  /** Shown instead of the table when there are no rows. */
  empty?: JSX.Element;
  /** A class for one row, such as a highlight for a row that just arrived. */
  rowClass?: (row: T) => string | undefined;
  /**
   * Where the whole row leads: a `paths` result. The row becomes one target a thumb can hit; its
   * own link in the cells stays the one keyboard stop, as the row link is hidden from both.
   */
  rowHref?: (row: T) => string | undefined;
}

/** A table with units in headers, right-aligned numbers and row rules. */
export function DataTable<T>(props: DataTableProps<T>) {
  const cellClass = (c: Column<T>) =>
    [c.numeric && "table__num", c.optional && "table__optional"].filter(Boolean).join(" ") ||
    undefined;
  return (
    <Show when={props.rows.length > 0 || props.empty === undefined} fallback={props.empty}>
      <div class="table-scroll">
        <table class={["table", { "table--rows": props.rowHref !== undefined }]}>
          <caption class={{ "visually-hidden": !props.showCaption, table__caption: true }}>
            {props.caption}
          </caption>
          <thead>
            <tr>
              <For each={props.columns}>
                {(c) => (
                  <th scope="col" class={cellClass(c)}>
                    <Show when={c.short} fallback={c.header}>
                      {(short) => (
                        <>
                          <span class="table__long">{c.header}</span>
                          <span class="table__short" aria-hidden="true">
                            {short()}
                          </span>
                        </>
                      )}
                    </Show>
                  </th>
                )}
              </For>
            </tr>
          </thead>
          <tbody>
            <For each={props.rows}>
              {(row, i) => (
                <tr class={props.rowClass?.(row)}>
                  <For each={props.columns}>
                    {(c, k) => (
                      <td class={cellClass(c)}>
                        {c.cell(row, i())}
                        <Show when={k() === 0 && props.rowHref?.(row)}>
                          {(href) => (
                            <Link
                              class="table__rowlink"
                              href={href()}
                              tabindex="-1"
                              aria-hidden="true"
                            />
                          )}
                        </Show>
                      </td>
                    )}
                  </For>
                </tr>
              )}
            </For>
          </tbody>
        </table>
      </div>
    </Show>
  );
}
