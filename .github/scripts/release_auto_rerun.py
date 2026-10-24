#!/usr/bin/env python3
"""发布类运行首轮失败、且失败全部属于外部瞬时错误时，自动 `gh run rerun --failed` 一次。

由 .github/workflows/release-auto-rerun.yml（workflow_run 触发）调用。
判定：除 Publish Release（组件不齐时按设计失败，是结果不是原因）外，每个失败 job 的日志都命中
TRANSIENT 模式才重跑；任何一个失败不像瞬时错误（编译错误、Docker 上下文缺文件等）就不动——
确定性错误重跑只会白等（v0.5.4 的 Docker 失败手动 Re-run 后原样失败）。
只处理第 1 次尝试，自动重跑至多一次。

用法：GH_TOKEN=… GH_REPO=owner/name python3 .github/scripts/release_auto_rerun.py <run-id> [--dry-run]
"""

import json
import os
import re
import subprocess
import sys

DERIVED_JOBS = {"Publish Release"}

TRANSIENT = re.compile(
    r"NSURLErrorDomain Code=-10(?:01|05|09)"  # 超时 / 连接中断 / 离线（notarytool、stapler）
    r"|A timestamp was expected but was not found"  # codesign 时间戳服务不可达
    r"|A server error occurred\. \(-19241\)"  # App Store Connect 上传服务端错误
    r"|The (?:staple and )?validate action failed! Error 68"
    r"|HTTP (?:500|502|503|504)\b"
    r"|ECONNRESET|ETIMEDOUT|Connection reset by peer"
    r"|TLS handshake timeout|net/http: request canceled"
)


def gh(*args: str) -> str:
    return subprocess.run(["gh", *args], check=True, capture_output=True, text=True).stdout


def main() -> int:
    run_id = sys.argv[1]
    dry_run = "--dry-run" in sys.argv[2:]
    repo = os.environ["GH_REPO"]
    run = json.loads(gh("api", f"repos/{repo}/actions/runs/{run_id}"))
    if run["run_attempt"] != 1 or run["conclusion"] != "failure":
        print(f"attempt {run['run_attempt']} / {run['conclusion']}: nothing to do")
        return 0
    jobs = json.loads(gh("api", f"repos/{repo}/actions/runs/{run_id}/attempts/1/jobs?per_page=100"))["jobs"]
    failed = [j for j in jobs if j["conclusion"] == "failure" and j["name"] not in DERIVED_JOBS]
    if not failed:
        print("only derived jobs failed; nothing to rerun")
        return 0
    for job in failed:
        log = subprocess.run(
            ["gh", "api", "--allow-escape-sequences", f"repos/{repo}/actions/jobs/{job['id']}/logs"],
            capture_output=True,
            text=True,
            errors="replace",
        ).stdout
        hit = TRANSIENT.search(log)
        if not hit:
            print(f"::notice::{job['name']} failed for a non-transient reason; not rerunning")
            return 0
        print(f"{job['name']}: transient failure ({hit.group(0)})")
    print(f"::notice::all {len(failed)} failed job(s) are transient; rerunning failed jobs of run {run_id}")
    if not dry_run:
        gh("run", "rerun", run_id, "--failed", "--repo", repo)
    return 0


if __name__ == "__main__":
    sys.exit(main())
