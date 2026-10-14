/**
 * Call `onChange` whenever the device pixel ratio moves, as when a window
 * crosses onto a display with a different scale.
 *
 * A resolution query only matches the ratio it was built for, so it is rebuilt
 * after every change. A resize observer does not fire for this: the element's
 * CSS size is unchanged while its backing store has become the wrong size.
 */
export function watchPixelRatio(onChange: () => void): () => void {
  if (typeof window === "undefined" || typeof window.matchMedia !== "function") {
    return () => undefined;
  }
  let query: MediaQueryList | null = null;
  const changed = () => {
    arm();
    onChange();
  };
  function arm(): void {
    query?.removeEventListener("change", changed);
    query = window.matchMedia(`(resolution: ${window.devicePixelRatio || 1}dppx)`);
    query.addEventListener("change", changed);
  }
  arm();
  return () => query?.removeEventListener("change", changed);
}
