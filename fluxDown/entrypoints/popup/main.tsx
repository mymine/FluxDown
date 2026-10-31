import { render } from "preact";
import { installErrorCapture, logWarn } from "@/utils/error-log";
import { initI18n } from "@/utils/i18n";
import { PopupApp } from "./PopupApp";

installErrorCapture("popup");

async function boot(): Promise<void> {
  try {
    await initI18n();
  } catch (e) {
    // 语言初始化失败时沿用默认语言，仍然渲染界面
    logWarn("[FluxDown] initI18n failed:", e);
  }
  render(<PopupApp />, document.getElementById("app") as HTMLElement);
}

void boot();
