import { createSignal } from "solid-js";
import "./SearchField.css";

export interface SearchFieldProps {
  /** Called with the trimmed query on Enter. Return false when nothing matched, to keep the text. */
  onSearch: (query: string) => boolean;
  id?: string;
  ref?: (el: HTMLInputElement) => void;
}

/** The search field. What a query finds is decided by the `onSearch` hook (src/search/). */
export function SearchField(props: SearchFieldProps) {
  const id = () => props.id ?? "search";
  const [invalid, setInvalid] = createSignal(false);
  const submit = (e: SubmitEvent) => {
    e.preventDefault();
    const input = (e.currentTarget as HTMLFormElement).elements.namedItem("q");
    if (!(input instanceof HTMLInputElement)) return;
    const query = input.value.trim();
    if (query === "") return;
    const found = props.onSearch(query);
    setInvalid(!found);
    if (found) {
      input.value = "";
      input.blur();
    }
  };
  return (
    <search class="search">
      <form onSubmit={submit}>
        <label class="visually-hidden" for={id()}>
          Search
        </label>
        <svg class="search__icon" viewBox="0 0 16 16" aria-hidden="true">
          <circle cx="7" cy="7" r="4.5" />
          <path d="M10.5 10.5 14 14" />
        </svg>
        <input
          ref={props.ref}
          id={id()}
          name="q"
          type="search"
          placeholder="Transaction, address or block"
          autocomplete="off"
          spellcheck={false}
          enterkeyhint="search"
          aria-invalid={invalid() ? "true" : undefined}
          aria-describedby={invalid() ? `${id()}-hint` : undefined}
          onInput={() => setInvalid(false)}
        />
        <span id={`${id()}-hint`} class="search__hint" role="status">
          {invalid()
            ? "Nothing matches. Enter a transaction ID, block hash, height or address."
            : ""}
        </span>
      </form>
    </search>
  );
}
