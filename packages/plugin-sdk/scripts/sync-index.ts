// Copies dist/manifest.mjs into the plugin index repo as vendor/fluxdown-plugin-manifest.mjs.
// Usage: bun scripts/sync-index.ts --index <path-to-fluxdown-plugin-index> [--check]
import { copyFileSync, existsSync, mkdirSync, readFileSync } from "node:fs";
import { join, resolve } from "node:path";

const args = process.argv.slice(2);
const flag = args.indexOf("--index");
const target = flag >= 0 ? args[flag + 1] : undefined;
if (!target) {
  console.error("usage: sync-index.ts --index <path-to-fluxdown-plugin-index> [--check]");
  process.exit(2);
}

const source = new URL("../dist/manifest.mjs", import.meta.url).pathname;
if (!existsSync(source)) {
  console.error("dist/manifest.mjs not found; run `bun run build` first");
  process.exit(2);
}
const dest = join(resolve(target), "vendor", "fluxdown-plugin-manifest.mjs");

if (args.includes("--check")) {
  const same = existsSync(dest) && readFileSync(dest).equals(readFileSync(source));
  if (!same) {
    console.error(`${dest} is out of date; run sync-index without --check`);
    process.exit(1);
  }
} else {
  mkdirSync(join(resolve(target), "vendor"), { recursive: true });
  copyFileSync(source, dest);
  console.log(`synced ${dest}`);
}
