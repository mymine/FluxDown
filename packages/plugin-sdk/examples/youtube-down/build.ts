// 生成 resolve.js（引擎只加载 manifest 指向的单个 classic script）：
//   vendor/yt-dlp-ejs 原样置顶（保留许可证头）→ 包进作用域，导出为全局 `ytSolver`；
//   src/ 用 esbuild 打成 IIFE 接在后面。
// 用法：bun examples/youtube-down/build.ts [--check]（--check 只比对入库的 resolve.js，不写文件）。
import { createHash } from "node:crypto";
import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import * as esbuild from "esbuild";

const root = dirname(fileURLToPath(import.meta.url));
const vendorDir = join(root, "vendor", "yt-dlp-ejs");
const output = join(root, "resolve.js");

/**
 * yt-dlp-ejs 0.8.0 发布物的 SHA3-512，取自 yt-dlp `extractor/youtube/jsc/_builtin/vendor/_info.py`
 * （yt-dlp 对同名文件做同样的校验）。升级时换文件、换哈希，并同步 src/player.js 的 EJS_VERSION。
 */
const VENDOR_SHA3_512: Record<string, string> = {
  "yt.solver.lib.min.js":
    "8420c259ad16e99ce004e4651ac1bcabb53b4457bf5668a97a9359be9a998a789fee8ab124ee17f91a2ea8fd84e0f2b2fc8eabcaf0b16a186ba734cf422ad053",
  "yt.solver.core.min.js":
    "ee5b307d07f55e91e4723edf5ac205cc877a474187849d757dc1322e38427b157a9d706d510c1723d3670f98e5a3f8cbcde77874a80406bd7204bc9fea30f283",
};

function vendor(name: string): string {
  const code = readFileSync(join(vendorDir, name), "utf8");
  const hash = createHash("sha3-512").update(code).digest("hex");
  if (hash !== VENDOR_SHA3_512[name]) throw new Error(`${name}: SHA3-512 不符（${hash.slice(0, 16)}…），vendor 文件被改动`);
  return code.replace(/\n+$/, "");
}

const bundled = await esbuild.build({
  entryPoints: [join(root, "src", "main.js")],
  bundle: true,
  write: false,
  format: "iife",
  platform: "neutral",
  target: "es2022",
  charset: "utf8",
  legalComments: "none",
  logLevel: "silent",
});
const plugin = bundled.outputFiles[0]?.text;
if (!plugin) throw new Error("esbuild 没有产出");

const text = [
  "// 生成文件，勿手改：源码在 src/，签名求解器在 vendor/yt-dlp-ejs/，由 build.ts 拼装（bun examples/youtube-down/build.ts）。",
  "const ytSolver = (() => {",
  vendor("yt.solver.lib.min.js"),
  "const { meriyah, astring } = lib;",
  vendor("yt.solver.core.min.js"),
  "return jsc;",
  "})();",
  plugin,
].join("\n");

if (process.argv.includes("--check")) {
  let current = "";
  try {
    current = readFileSync(output, "utf8");
  } catch {
    current = "";
  }
  if (current !== text) {
    console.error("examples/youtube-down/resolve.js 与 src/ + vendor/ 不一致，请运行 bun examples/youtube-down/build.ts");
    process.exit(1);
  }
  console.log("examples/youtube-down/resolve.js up to date");
} else {
  writeFileSync(output, text);
  console.log(`wrote examples/youtube-down/resolve.js (${text.length} chars)`);
}
