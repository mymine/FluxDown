#!/usr/bin/env bun
// 为嗅探规则包 / 吊销列表生成签名信封。
//
// 用法：
//   bun scripts/sniff-rules/sign.ts --kind official|community|revocations \
//     --key <私钥.pem> --key-id <keyId> [--key <次私钥.pem> --key-id <次keyId> ...] \
//     --in <payload.json> --out <signed.json>
//
// --key / --key-id 成对按顺序重复给出即多签（密钥轮换期新旧并签，agent 任一签名验过即接受）。
//
// 签名对象是 payload 文件的原始字节（不重新序列化），信封格式：
//   {"payload": base64(标准), "signatures": [{"keyId": "...", "sig": base64}]}
import { createPrivateKey, sign } from "node:crypto";
import { readFileSync, writeFileSync } from "node:fs";

function arg(name: string): string | undefined {
  const i = process.argv.indexOf(name);
  return i >= 0 ? process.argv[i + 1] : undefined;
}

function args(name: string): string[] {
  return process.argv.flatMap((value, i) => {
    const next = process.argv[i + 1];
    return value === name && next !== undefined ? [next] : [];
  });
}

const kind = arg("--kind");
const keyPaths = args("--key");
const keyIds = args("--key-id");
const input = arg("--in");
const output = arg("--out");
if (!kind || !["official", "community", "revocations"].includes(kind) || keyPaths.length === 0 || keyPaths.length !== keyIds.length || !input || !output) {
  console.error(
    "用法: sign.ts --kind official|community|revocations --key <pem> --key-id <id> [--key <pem> --key-id <id> ...] --in <payload.json> --out <signed.json>",
  );
  process.exit(2);
}

const payload = readFileSync(input);
let doc: { format?: unknown; packId?: unknown };
try {
  doc = JSON.parse(payload.toString("utf8"));
} catch (e) {
  console.error(`payload 不是合法 JSON: ${(e as Error).message}`);
  process.exit(1);
}
const expectedFormat = kind === "revocations" ? "fluxdown-sniff-revocations" : "fluxdown-sniff-pack";
if (doc.format !== expectedFormat) {
  console.error(`payload.format 应为 ${expectedFormat}`);
  process.exit(1);
}
if (kind !== "revocations" && doc.packId !== kind) {
  console.error(`payload.packId 应为 ${kind}`);
  process.exit(1);
}

const envelope = {
  payload: payload.toString("base64"),
  signatures: keyPaths.map((keyPath, i) => ({
    keyId: keyIds[i],
    sig: sign(null, payload, createPrivateKey(readFileSync(keyPath))).toString("base64"),
  })),
};
writeFileSync(output, JSON.stringify(envelope));
console.log(`已写入 ${output}（${payload.length} 字节 payload）`);
