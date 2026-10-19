#!/usr/bin/env bun
// 生成嗅探规则包签名密钥（Ed25519）。
//
// 用法：
//   bun scripts/sniff-rules/keygen.ts --id release-2026-1
//   bun scripts/sniff-rules/keygen.ts --id release-2026-1 --print-public   # 只从已有 pem 打印公钥
//
// 私钥写入 ~/.fluxdown-signing/sniff/<id>.pem（权限 0600），已存在时拒绝覆盖；
// 公钥以 Rust 数组字面量输出，填入 native/agent/src/sniff_rules/keys.rs。
import { type KeyObject, createPrivateKey, createPublicKey, generateKeyPairSync } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";

function arg(name: string): string | undefined {
  const i = process.argv.indexOf(name);
  return i >= 0 ? process.argv[i + 1] : undefined;
}

const id = arg("--id");
if (!id || !/^[a-z0-9][a-z0-9._-]*$/.test(id)) {
  console.error("用法: keygen.ts --id <keyId> [--print-public]");
  process.exit(2);
}

const dir = join(homedir(), ".fluxdown-signing", "sniff");
const pemPath = join(dir, `${id}.pem`);

function rawPublic(publicKey: KeyObject): Buffer {
  const x = publicKey.export({ format: "jwk" }).x;
  if (!x) throw new Error("无法导出公钥");
  return Buffer.from(x, "base64url");
}

function rustArray(bytes: Buffer): string {
  const rows: string[] = [];
  for (let i = 0; i < bytes.length; i += 8) {
    rows.push(
      "    " +
        Array.from(bytes.subarray(i, i + 8))
          .map((b) => `0x${b.toString(16).padStart(2, "0")}`)
          .join(", ") +
        ",",
    );
  }
  return `[\n${rows.join("\n")}\n]`;
}

if (process.argv.includes("--print-public")) {
  if (!existsSync(pemPath)) {
    console.error(`私钥不存在: ${pemPath}`);
    process.exit(1);
  }
  const publicKey = createPublicKey(createPrivateKey(readFileSync(pemPath)));
  console.log(`// keyId: ${id}\n${rustArray(rawPublic(publicKey))}`);
  process.exit(0);
}

if (existsSync(pemPath)) {
  console.error(`拒绝覆盖已存在的私钥: ${pemPath}（用 --print-public 查看其公钥）`);
  process.exit(1);
}
mkdirSync(dir, { recursive: true, mode: 0o700 });
const { publicKey, privateKey } = generateKeyPairSync("ed25519");
writeFileSync(pemPath, privateKey.export({ format: "pem", type: "pkcs8" }), { mode: 0o600, flag: "wx" });
console.log(`私钥已写入 ${pemPath}（请自行备份，不要进仓库）`);
console.log(`// keyId: ${id}\n${rustArray(rawPublic(publicKey))}`);
