import { lazy } from "solid-js";
import { AppShell } from "./shell/AppShell";
import { ThemeProvider } from "./theme/ThemeProvider";

/**
 * `?gallery` swaps the shell for the base-layer gallery — a dev review surface.
 *
 * Imported lazily so it is a chunk of its own: the gallery pulls in every
 * primitive and every lucide glyph it paints, and none of that belongs in the
 * bundle the app opens with.
 */
const Gallery = lazy(() => import("./theme/Gallery").then((mod) => ({ default: mod.Gallery })));

const isGallery =
  typeof window !== "undefined" && new URLSearchParams(window.location.search).has("gallery");

export function App() {
  return <ThemeProvider>{isGallery ? <Gallery /> : <AppShell />}</ThemeProvider>;
}
