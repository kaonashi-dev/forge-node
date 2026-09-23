/** Writes the in-app reduce-motion choice; `base.css` removes every transition under it. */
export function applyReduceMotion(reduce: boolean): void {
  const root = document.documentElement;
  if (reduce) root.dataset.reduceMotion = "true";
  else delete root.dataset.reduceMotion;
}
