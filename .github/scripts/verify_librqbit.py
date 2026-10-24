#!/usr/bin/env python3
"""校验 librqbit 解析自钉死 rev 的 MSE fork（release.yml 的 verify-engine 与 ci.yml 共用）。

用法：python3 .github/scripts/verify_librqbit.py [--github-output]
读取 `cargo metadata --locked` 结果；失败时以非零退出并打印原因。
版本号不在此写死：fork 升级只改 Cargo.toml / Cargo.lock，本检查只守「来源」不变式
（曾因写死 9.0.1 让依赖升级后的发布门禁失败，见 v0.5.6）。
--github-output：把解析出的 manifest 路径写到 $GITHUB_OUTPUT（manifest=...），供后续 MSE 单测使用。
"""

import json
import os
import pathlib
import subprocess
import sys

FORK_PREFIX = "git+https://github.com/zerx-lab/rqbit?rev="


def main() -> int:
    metadata = json.loads(
        subprocess.run(
            ["cargo", "metadata", "--locked", "--format-version", "1"],
            check=True,
            capture_output=True,
            text=True,
        ).stdout
    )
    packages = [p for p in metadata["packages"] if p["name"] == "librqbit"]
    if len(packages) != 1:
        print(f"expected one librqbit package, found {len(packages)}", file=sys.stderr)
        return 1
    package = packages[0]
    manifest = pathlib.Path(package["manifest_path"]).as_posix()
    source = package["source"] or ""
    if not source.startswith(FORK_PREFIX):
        print(f"librqbit must resolve from a pinned MSE fork revision, got: {source}", file=sys.stderr)
        return 1
    requested_rev, separator, resolved_rev = source.removeprefix(FORK_PREFIX).partition("#")
    if not separator or requested_rev != resolved_rev:
        print(f"librqbit requested and resolved revisions differ: {source}", file=sys.stderr)
        return 1
    if "/native/engine/vendor/" in manifest:
        print(f"librqbit must not resolve from the vendored copy: {manifest}", file=sys.stderr)
        return 1
    if "--github-output" in sys.argv[1:]:
        with open(os.environ["GITHUB_OUTPUT"], "a", encoding="utf-8") as output:
            print(f"manifest={manifest}", file=output)
    print(f"verified fork librqbit {package['version']}: {manifest} ({source})")
    return 0


if __name__ == "__main__":
    sys.exit(main())
