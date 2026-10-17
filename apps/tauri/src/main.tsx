import { render } from "solid-js/web";
import { App } from "./app/App";
import { applyThemeBase } from "./theme/ThemeProvider";
import { recallTheme } from "./theme/lastTheme";
import "./theme/tokens.css";
import "./styles/index.css";
import "@forge-node/file-workbench/style.css";
import "./features/files/explorer/fileSurface.css";

applyThemeBase(recallTheme() ?? "calm-dark");

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
