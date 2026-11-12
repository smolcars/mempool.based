import { For, Show } from "solid-js";
import type { JSX } from "@solidjs/web";
import "./DataTable.css";

export interface Column<T> {
  /** Header text. Units go here ("Value, BTC"), so cells hold bare numbers. */
  header: string;
  cell: (row: T, index: number) => JSX.Element;
  /** Right-aligned numeric column. */
  numeric?: boolean;
  /** Dropped on phones. Mark the least important columns. */
  optional?: boolean;
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
}

/** A table with units in headers, right-aligned numbers and row rules. */
export function DataTable<T>(props: DataTableProps<T>) {
  const cellClass = (c: Column<T>) =>
    [c.numeric && "table__num", c.optional && "table__optional"].filter(Boolean).join(" ") ||
    undefined;
  return (
    <Show when={props.rows.length > 0 || props.empty === undefined} fallback={props.empty}>
      <div class="table-scroll">
        <table class="table">
          <caption class={{ "visually-hidden": !props.showCaption, table__caption: true }}>
            {props.caption}
          </caption>
          <thead>
            <tr>
              <For each={props.columns}>
                {(c) => (
                  <th scope="col" class={cellClass(c)}>
                    {c.header}
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
                    {(c) => <td class={cellClass(c)}>{c.cell(row, i())}</td>}
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
