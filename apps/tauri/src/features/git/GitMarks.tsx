/** The warning triangle an unresolved conflict wears beside its word. */
export function ConflictMark(props: { size?: number }) {
  return (
    <svg
      class="git-conflict-mark"
      width={props.size ?? 13}
      height={props.size ?? 13}
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      stroke-width="1.5"
      stroke-linecap="round"
      aria-hidden="true"
    >
      <path d="M8 5.4v3.4M8 11.2v.2" />
      <path d="M6.9 2.4L1.7 11.4a1.3 1.3 0 001.1 2h10.4a1.3 1.3 0 001.1-2L9.1 2.4a1.3 1.3 0 00-2.2 0z" />
    </svg>
  );
}
