/** Writes the in-app reduce-motion choice; `base.css` removes every transition under it. */
export function applyReduceMotion(reduce: boolean): void {
  const root = document.documentElement;
  if (reduce) root.dataset.reduceMotion = "true";
  else delete root.dataset.reduceMotion;
}

/**
 * Pauses every loop while the window is hidden: a ring that spins behind a
 * minimised window is a compositor waking at 60fps for nobody.
 */
export function pauseLoopsWhileHidden(): () => void {
  const sync = () => {
    if (document.hidden) document.documentElement.dataset.windowHidden = "true";
    else delete document.documentElement.dataset.windowHidden;
  };
  sync();
  document.addEventListener("visibilitychange", sync);
  return () => document.removeEventListener("visibilitychange", sync);
}

/**
 * Plays a one-shot animation class on `element` because something happened.
 * Removing and re-adding the class restarts it; the one forced style read is
 * the price of a restart, paid per event and never per frame.
 */
export function playOnce(element: Element | undefined, name: string): void {
  if (!element) return;
  element.classList.remove(name);
  void element.getBoundingClientRect();
  element.classList.add(name);
  element.addEventListener("animationend", () => element.classList.remove(name), { once: true });
}

/** Whether a record was minted moments ago, for an entrance that marks its arrival. */
export function justCreated(createdAt: string, now = Date.now(), window = 3000): boolean {
  const at = Date.parse(createdAt);
  return Number.isFinite(at) && now - at >= 0 && now - at < window;
}
