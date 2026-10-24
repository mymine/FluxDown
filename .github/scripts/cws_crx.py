#!/usr/bin/env python3
"""把 Chrome 网上应用店已发布版本的商店签名 CRX 搬到同版本 GitHub Release。

由 .github/workflows/cws-crx.yml（定时 + 手动）调用。商店审核异步，release.yml 推 tag
时拿不到新版本的商店签名包，只能事后轮询：
  1. 问商店更新接口（与 Chrome 扩展更新器同一个 gupdate 协议）当前已发布版本与下载地址；
  2. 对应 release `v<版本>` 不存在或已有 `-chrome.crx` → 无事可做；
  3. 下载 CRX，校验商店给出的 sha256 / 大小、CRX3 头部、扩展 ID 与包内 manifest 版本；
  4. 原样写成 `FluxDown-<版本>-chrome.crx`，由 workflow 上传 GitHub Release 与 OSS。

直接发 .crx：用户下载后拖进 chrome://extensions 即装。Chrome 可能把网页直接下载的 .crx
当作安装请求而拒绝（「无法从该网站添加…」），官网与文档提示改用「链接另存为」。

另外：最新稳定扩展 release 晚于商店版本超过 STALE_DAYS 天 → 打 warning（商店上传失败、
审核被拒都只在 release.yml 里 continue-on-error，这里是唯一的持续提醒）。

用法：GH_TOKEN=… GH_REPO=owner/name python3 .github/scripts/cws_crx.py <输出目录>
结果写入 $GITHUB_OUTPUT：upload=true|false，tag，file。
"""

from __future__ import annotations

import hashlib
import io
import json
import os
import subprocess
import sys
import urllib.parse
import urllib.request
import zipfile
from datetime import datetime, timedelta, timezone
from pathlib import Path
from xml.etree import ElementTree

EXTENSION_ID = "meleenglfggcmcajknpeeeiobnpfmahc"
# 更新接口按 prodversion 过滤 minimum_chrome_version；取足够新的版本号，避免被误判为不兼容
PRODVERSION = "140.0.0.0"
UPDATE_URL = "https://clients2.google.com/service/update2/crx"
GUPDATE_NS = "{http://www.google.com/update2/response}"
ASSET_SUFFIX = "-chrome.crx"
EXTENSION_SENTINEL = "SHA256SUMS-extension.txt"
STALE_DAYS = 7
HTTP_TIMEOUT = 60
USER_AGENT = "FluxDown-Release-Bot"


class CrxError(Exception):
    pass


def http_get(url: str) -> bytes:
    request = urllib.request.Request(url, headers={"User-Agent": USER_AGENT})
    with urllib.request.urlopen(request, timeout=HTTP_TIMEOUT) as response:
        return response.read()


def gh(*args: str) -> str:
    return subprocess.run(["gh", *args], check=True, capture_output=True, text=True).stdout


def store_update() -> dict[str, str]:
    """商店当前已发布版本：{version, codebase, sha256, size}。"""
    x = urllib.parse.urlencode({"id": EXTENSION_ID, "v": "0.0.0", "uc": ""})
    query = urllib.parse.urlencode({"prodversion": PRODVERSION, "acceptformat": "crx3", "x": x})
    root = ElementTree.fromstring(http_get(f"{UPDATE_URL}?{query}"))
    app = root.find(f"{GUPDATE_NS}app")
    if app is None or app.get("status") != "ok":
        raise CrxError(f"store app status: {None if app is None else app.get('status')}")
    check = app.find(f"{GUPDATE_NS}updatecheck")
    if check is None or check.get("status") != "ok":
        raise CrxError(f"store updatecheck status: {None if check is None else check.get('status')}")
    fields = {key: check.get(key, "") for key in ("version", "codebase", "hash_sha256", "size")}
    if not fields["version"] or not fields["codebase"].startswith("https://"):
        raise CrxError(f"store updatecheck incomplete: {fields}")
    return fields


def version_key(version: str) -> tuple[int, ...]:
    return tuple(int(part) for part in version.split("."))


def stable_tag_version(tag: str) -> tuple[int, ...] | None:
    """`vX.Y.Z` / 历史 `extension-vX.Y.Z` → 版本元组；预发布或其他 tag → None。"""
    bare = tag.removeprefix("extension-").removeprefix("v")
    try:
        return version_key(bare)
    except ValueError:
        return None


def read_varint(data: bytes, pos: int) -> tuple[int, int]:
    value = shift = 0
    while True:
        if pos >= len(data) or shift > 63:
            raise CrxError("truncated varint in CRX header")
        byte = data[pos]
        pos += 1
        value |= (byte & 0x7F) << shift
        if byte < 0x80:
            return value, pos
        shift += 7


def proto_fields(data: bytes) -> list[tuple[int, bytes]]:
    """解析 protobuf 消息里的 length-delimited 字段（CRX3 头部只用到这一种）。"""
    fields = []
    pos = 0
    while pos < len(data):
        tag, pos = read_varint(data, pos)
        number, wire = tag >> 3, tag & 7
        if wire == 0:
            _, pos = read_varint(data, pos)
        elif wire == 2:
            length, pos = read_varint(data, pos)
            end = pos + length
            if end > len(data):
                raise CrxError("truncated field in CRX header")
            fields.append((number, data[pos:end]))
            pos = end
        else:
            raise CrxError(f"unexpected wire type {wire} in CRX header")
    return fields


def crx_id(digest: bytes) -> str:
    """Chrome 扩展 ID：前 16 字节的十六进制逐位映射到 a-p。"""
    return "".join(chr(ord("a") + int(c, 16)) for c in digest[:16].hex())


def verify_crx(blob: bytes, version: str) -> None:
    """CRX3 格式、扩展 ID（声明值与签名公钥一致）、包内 manifest 版本。"""
    if blob[:4] != b"Cr24" or int.from_bytes(blob[4:8], "little") != 3:
        raise CrxError("not a CRX3 file")
    header_len = int.from_bytes(blob[8:12], "little")
    header = blob[12 : 12 + header_len]
    if len(header) != header_len:
        raise CrxError("truncated CRX header")
    fields = proto_fields(header)
    signed = [value for number, value in fields if number == 10000]
    if len(signed) != 1:
        raise CrxError("CRX header lacks signed_header_data")
    declared = [value for number, value in proto_fields(signed[0]) if number == 1]
    if len(declared) != 1 or crx_id(declared[0]) != EXTENSION_ID:
        raise CrxError("CRX declares a different extension id")
    # 声明的 ID 必须对应某个 RSA 证明的公钥（Chrome 安装时同样这样认定开发者身份）
    rsa_keys = [
        key
        for number, proof in fields
        if number == 2
        for key_number, key in proto_fields(proof)
        if key_number == 1
    ]
    if not any(crx_id(hashlib.sha256(key).digest()) == EXTENSION_ID for key in rsa_keys):
        raise CrxError("no CRX key proof matches the extension id")
    with zipfile.ZipFile(io.BytesIO(blob[12 + header_len :])) as archive:
        manifest = json.loads(archive.read("manifest.json"))
    if manifest.get("version") != version:
        raise CrxError(f"manifest version {manifest.get('version')} != store version {version}")


def download_crx(update: dict[str, str]) -> bytes:
    blob = http_get(update["codebase"])
    if update["size"] and len(blob) != int(update["size"]):
        raise CrxError(f"size {len(blob)} != store size {update['size']}")
    if update["hash_sha256"] and hashlib.sha256(blob).hexdigest() != update["hash_sha256"].lower():
        raise CrxError("sha256 mismatch with store response")
    verify_crx(blob, update["version"])
    return blob


def warn_if_store_stale(releases: list[dict], store_version: tuple[int, ...]) -> None:
    for release in releases:
        if release["isDraft"] or release["isPrerelease"]:
            continue
        tag_version = stable_tag_version(release["tagName"])
        if tag_version is None or tag_version <= store_version:
            continue
        published = datetime.fromisoformat(release["publishedAt"].replace("Z", "+00:00"))
        if datetime.now(timezone.utc) - published < timedelta(days=STALE_DAYS):
            continue
        assets = gh("release", "view", release["tagName"], "--json", "assets", "--jq", ".assets[].name").split()
        if EXTENSION_SENTINEL in assets:
            print(
                f"::warning::扩展 {release['tagName']} 发布超过 {STALE_DAYS} 天，商店仍是 "
                f"{'.'.join(map(str, store_version))}：检查 Chrome Web Store 上传 / 审核状态"
            )
            return


def write_outputs(**values: str) -> None:
    path = os.environ.get("GITHUB_OUTPUT")
    lines = "".join(f"{key}={value}\n" for key, value in values.items())
    if path:
        with open(path, "a", encoding="utf-8") as fh:
            fh.write(lines)
    print(lines, end="")


def main() -> int:
    out_dir = Path(sys.argv[1])
    update = store_update()
    version = update["version"]
    print(f"store version: {version}")

    releases = json.loads(
        gh(
            "release", "list", "--limit", "50",
            "--json", "tagName,isDraft,isPrerelease,publishedAt",
        )
    )
    warn_if_store_stale(releases, version_key(version))

    tag = next(
        (
            r["tagName"]
            for r in releases
            if not r["isDraft"] and r["tagName"] in (f"v{version}", f"extension-v{version}")
        ),
        None,
    )
    if tag is None:
        print(f"no published release for store version {version}; skipping")
        write_outputs(upload="false")
        return 0
    assets = gh("release", "view", tag, "--json", "assets", "--jq", ".assets[].name").split()
    if any(name.endswith(ASSET_SUFFIX) for name in assets):
        print(f"{tag} already has a store-signed CRX; skipping")
        write_outputs(upload="false")
        return 0

    blob = download_crx(update)
    out_dir.mkdir(parents=True, exist_ok=True)
    asset = out_dir / f"FluxDown-{version}{ASSET_SUFFIX}"
    asset.write_bytes(blob)
    print(f"verified store CRX {len(blob)} bytes → {asset}")
    write_outputs(upload="true", tag=tag, file=str(asset))
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except CrxError as err:
        print(f"::error::{err}")
        sys.exit(1)
