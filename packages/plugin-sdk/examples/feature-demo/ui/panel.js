// 嵌入式桌面脚本（活动栏页）：gpui-shell 运行，无 ShellRoot，浮层用 fluxdown.toast。
import { View, div } from "gpui-kit";
import { v_flex, h_flex } from "gpui-base";
import { Button } from "gpui-component";
import { identity, taskId, call, invoke, theme, locale, toast } from "fluxdown";

export default class Panel extends View {
  init(_props, cx) {
    this.output = "点击按钮测试 fluxdown 模块";
    this.cx = cx;
  }

  run(label, work) {
    this.cx.spawn(async (cx) => {
      try {
        const result = await work();
        this.output = `${label}：${JSON.stringify(result, null, 2).slice(0, 800)}`;
      } catch (e) {
        this.output = `${label} 失败：${e && e.message ? e.message : e}`;
      }
      cx.notify();
    });
  }

  render(cx) {
    const t = theme();
    return v_flex()
      .child(`桌面脚本面板 · ${identity()} · 语言 ${locale()} · taskId ${taskId()}`)
      .child(`主题 revision ${t.revision} · colors.primary = ${t.tokens["colors.primary"]}`)
      .child(
        h_flex()
          .child(Button.new("invoke-stats").label("invoke stats").primary().on_click(() => this.run("invoke stats", () => invoke("stats"))))
          .child(Button.new("invoke-echo").label("invoke echo").on_click(() => this.run("invoke echo", () => invoke("echo", { hi: 1 }))))
          .child(Button.new("call-list").label("call daemon.task.list").on_click(() => this.run("daemon.task.list", async () => /** @type {unknown[]} */ (await call("daemon.task.list", {})).length)))
          .child(
            Button.new("call-denied")
              .label("call 非白名单（应拒绝）")
              .danger()
              .on_click(() => this.run("system.shutdown", () => call("system.shutdown"))),
          )
          .child(
            Button.new("fetch")
              .label("fetch httpbin（已授权主机）")
              .on_click(() => this.run("fetch httpbin.org", async () => (await fetch("https://httpbin.org/uuid")).text())),
          )
          .child(
            Button.new("fetch-denied")
              .label("fetch example.com（未授权）")
              .on_click(() => this.run("fetch example.com", async () => (await fetch("https://example.com/")).status)),
          )
          .child(Button.new("toast").label("fluxdown.toast").on_click(() => toast("success", "来自桌面脚本的通知"))),
      )
      .child(div().child(this.output));
  }
}
