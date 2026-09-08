import { render } from "solid-js/web";
import { App } from "./App";
import { applyThemeBase } from "./theme/ThemeProvider";
import "./theme/tokens.css";
import "./ui/ui.css";
import "./styles/index.css";

applyThemeBase("gruvbox-hard");

const isMac =
  /mac/i.test(navigator.platform) ||
  /mac/i.test(navigator.userAgent) ||
  (navigator as Navigator & { userAgentData?: { platform: string } }).userAgentData?.platform ===
    "macOS";
if (isMac) {
  document.documentElement.dataset.platform = "macos";
}

const root = document.getElementById("root");
if (!root) {
  throw new Error("root element missing");
}

render(() => <App />, root);
