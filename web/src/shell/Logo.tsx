/** The mark: a block of settled strata, the top layer still arriving. The tile takes the network color. */
export function Logo() {
  return (
    <svg class="logo" viewBox="0 0 32 32" aria-hidden="true">
      <rect class="logo__tile" x="1" y="1" width="30" height="30" rx="7" />
      <rect class="logo__layer" x="6" y="20" width="20" height="5" rx="1.5" />
      <rect class="logo__layer logo__layer--mid" x="6" y="13.5" width="20" height="5" rx="1.5" />
      <rect class="logo__layer logo__layer--top" x="12" y="7" width="14" height="5" rx="1.5" />
    </svg>
  );
}
