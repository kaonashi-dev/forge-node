import { render } from "solid-js/web";
import { App } from "./app/App";
import { applyAppearance } from "./theme/ThemeProvider";
import { recallFinishes, recallTheme } from "./theme/lastTheme";
import { pauseLoopsWhileHidden } from "./theme/motion";
import "./theme/tokens.css";
import "./styles/index.css";
import "@forge-node/file-workbench/style.css";
import "./features/files/explorer/fileSurface.css";

applyAppearance(recallTheme() ?? "calm-dark", recallFinishes());
pauseLoopsWhileHidden();

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
