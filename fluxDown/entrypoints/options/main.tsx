import { render } from "preact";
import "@/ui/theme/tokens.css";
import "@/ui/theme/base.css";
import { installErrorCapture, logError } from "@/utils/error-log";
import { initI18n } from "@/utils/i18n";
import { App } from "./App";
import "./options.css";

installErrorCapture("options");

const root = document.getElementById("app");
if (!root) throw new Error("#app mount point missing");

// 语言先初始化再首次渲染，避免首屏闪一下英文。
initI18n()
  .catch((e) => logError("[FluxDown Options] initI18n failed:", e))
  .finally(() => render(<App />, root));
