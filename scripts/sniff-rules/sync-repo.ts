#!/usr/bin/env bun
// 与嗅探规则仓 zerx-lab/fluxdown-sniff-rules 双向同步。
//
// 用法：
//   bun scripts/sniff-rules/sync-repo.ts vendor --repo <规则仓路径> [--check]
//     把扩展的规则校验 / 求值器打包为 <规则仓>/vendor/fluxdown-sniff-rules.mjs
//     （改了 fluxDown/rules/{schema,validate,engine,glob,url}.ts 后必须重跑）。
//   bun scripts/sniff-rules/sync-repo.ts seed --repo <规则仓路径> [--check]
//     把规则仓已发布的 dist/official.json 载荷写成 fluxDown/rules/official.json（随扩展的官方包快照）。
//
// --check：只比对，不一致时退出码 1。
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";

function arg(name: string): string | undefined {
  const i = process.argv.indexOf(name);
  return i >= 0 ? process.argv[i + 1] : undefined;
}

const mode = process.argv[2];
const repoArg = arg("--repo");
const check = process.argv.includes("--check");
if ((mode !== "vendor" && mode !== "seed") || !repoArg) {
  console.error("用法: sync-repo.ts vendor|seed --repo <规则仓路径> [--check]");
  process.exit(2);
}
const repo = resolve(repoArg);
const fluxDownRoot = resolve(import.meta.dir, "../..");

async function vendorText(): Promise<string> {
  const built = await Bun.build({
    entrypoints: [join(import.meta.dir, "vendor-entry.ts")],
    target: "node",
    format: "esm",
  });
  if (!built.success) {
    for (const log of built.logs) console.error(log);
    process.exit(1);
  }
  const header =
    "// 由 FluxDown 仓库 scripts/sniff-rules/sync-repo.ts vendor 生成，勿手改。\n" +
    "// 源：fluxDown/rules/{schema,validate,engine,glob,url}.ts 与 builtin.json。\n";
  return header + (await built.outputs[0].text());
}

function seedText(): string {
  const envelopePath = join(repo, "dist", "official.json");
  if (!existsSync(envelopePath)) {
    console.error(`未找到 ${envelopePath}：规则仓尚未发布官方包`);
    process.exit(1);
  }
  const envelope = JSON.parse(readFileSync(envelopePath, "utf8")) as { payload?: unknown };
  if (typeof envelope.payload !== "string") {
    console.error("dist/official.json 不是签名信封");
    process.exit(1);
  }
  const payload = JSON.parse(Buffer.from(envelope.payload, "base64").toString("utf8")) as {
    format?: unknown;
    packId?: unknown;
  };
  if (payload.format !== "fluxdown-sniff-pack" || payload.packId !== "official") {
    console.error("dist/official.json 的载荷不是官方规则包");
    process.exit(1);
  }
  return `${JSON.stringify(payload, null, 2)}\n`;
}

const [target, text] =
  mode === "vendor"
    ? [join(repo, "vendor", "fluxdown-sniff-rules.mjs"), await vendorText()]
    : [join(fluxDownRoot, "fluxDown", "rules", "official.json"), seedText()];

const current = existsSync(target) ? readFileSync(target, "utf8") : null;
if (check) {
  if (current !== text) {
    console.error(`${target} 已过期：去掉 --check 重新运行`);
    process.exit(1);
  }
  console.log(`${target} 已是最新`);
} else if (current === text) {
  console.log(`${target} 无变化`);
} else {
  mkdirSync(dirname(target), { recursive: true });
  writeFileSync(target, text);
  console.log(`已写入 ${target}`);
}
