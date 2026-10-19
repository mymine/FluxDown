// 独立窗口桌面脚本：层级 Root(ShellRoot(...))，可用 gpui-shell 浮层（toast / dialog）。
// 同时演示原生能力：文件读写（不逐次审计）、跨主机 fetch 授权。
import { View, div } from "gpui-kit";
import { v_flex, h_flex } from "gpui-base";
import { Button } from "gpui-component";
import { identity, invoke, theme, toast } from "fluxdown";
import { writeFile, readFile } from "fs/promises";
import { homedir } from "os";
import { join } from "path";

export default class DemoWindow extends View {
  init(_props, cx) {
    this.cx = cx;
    this.output = "独立窗口：可使用 window.push_toast / open_dialog";
  }

  run(label, work) {
    this.cx.spawn(async (cx) => {
      try {
        const result = await work();
        this.output = `${label}：${typeof result === "string" ? result : JSON.stringify(result)}`;
      } catch (e) {
        this.output = `${label} 失败：${e && e.message ? e.message : e}`;
      }
      cx.notify();
    });
  }

  render(cx) {
    const file = join(homedir(), "fluxdown-feature-demo.txt");
    return v_flex()
      .child(`独立窗口 · ${identity()} · 主题 revision ${theme().revision}`)
      .child(
        h_flex()
          .child(
            Button.new("shell-toast")
              .label("gpui-shell toast")
              .primary()
              .on_click(() => window.push_toast({ title: "Shell toast", description: "来自独立窗口", level: "success" })),
          )
          .child(
            Button.new("dialog")
              .label("打开对话框")
              .on_click(() => window.open_dialog(() => div().child("这是 gpui-shell 对话框，按 Esc 关闭"))),
          )
          .child(Button.new("flux-toast").label("fluxdown.toast").on_click(() => toast("info", "fluxdown.toast 也可用"))),
      )
      .child(
        h_flex()
          .child(Button.new("invoke").label("invoke stats").on_click(() => this.run("stats", () => invoke("stats"))))
          .child(
            Button.new("write")
              .label("写主目录文件")
              .on_click(() => this.run("write", async () => {
                await writeFile(file, `feature-demo ${new Date().toISOString()}\n`);
                return file;
              })),
          )
          .child(Button.new("read").label("读回文件").on_click(() => this.run("read", () => readFile(file, "utf8")))),
      )
      .child(div().child(this.output));
  }
}
