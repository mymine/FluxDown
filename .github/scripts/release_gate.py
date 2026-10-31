#!/usr/bin/env python3
"""release.yml 的发布前置判定（changes job 调用）。依赖 gh CLI（GH_TOKEN / GH_REPO）。

子命令：
  ci-gate --sha SHA [--timeout-min N]
      等待该提交最近一次 CI（ci.yml）运行结束并要求 success。CI 仍在跑就轮询等待（推 tag
      紧跟合入时不与 CI 抢跑）；红 / 取消 / 超时未出现即失败——在建草稿、开始构建之前停下。
  find-rehearsal --tag TAG --sha SHA --expected app,server,... [--github-output]
      找同一提交、同一 tag 的演练运行（run-name「Release 演练 <tag>（不发布）」），按其仍存在的
      rehearsal-sums-<组件> 完成哨兵判定哪些组件可直接复用演练产物（server 另需 docker-digest）。
      输出 reuse_run=<run id> 与 reuse=<逗号分隔组件>；没有可复用的演练时两者为空，流水线照常构建。
      浏览器扩展构建只需一两分钟且商店提交与构建同 job，恒重建，不参与复用。
"""

import argparse
import json
import os
import subprocess
import sys
import time

REUSABLE = ("app", "server", "cli", "mobile", "ios")


def gh_api(path: str) -> dict:
    out = subprocess.run(["gh", "api", path], check=True, capture_output=True, text=True).stdout
    return json.loads(out)


def repo() -> str:
    return os.environ["GH_REPO"]


def ci_gate(sha: str, timeout_min: int) -> int:
    deadline = time.monotonic() + timeout_min * 60
    while True:
        runs = gh_api(f"repos/{repo()}/actions/workflows/ci.yml/runs?head_sha={sha}&per_page=20")["workflow_runs"]
        runs.sort(key=lambda r: r["created_at"], reverse=True)
        if runs:
            run = runs[0]
            if run["status"] == "completed":
                if run["conclusion"] == "success":
                    print(f"CI {run['html_url']} succeeded for {sha}")
                    return 0
                print(
                    f"::error::提交 {sha} 的最近一次 CI 结论为 {run['conclusion']}（{run['html_url']}）；"
                    "修复并合入后重新打 tag，或在发布分支上 `gh workflow run ci.yml --ref <分支>` 跑全量 CI"
                )
                return 1
            print(f"CI {run['html_url']} is {run['status']}; waiting…")
        else:
            print(f"No CI run for {sha} yet; waiting…")
        if time.monotonic() > deadline:
            print(
                f"::error::{timeout_min} 分钟内提交 {sha} 没有成功结束的 CI；"
                "请 `gh workflow run ci.yml --ref <发布分支>` 跑全量 CI 后用 Re-run 重试本次运行"
            )
            return 1
        time.sleep(30)


def find_rehearsal(tag: str, sha: str, expected: list[str]) -> tuple[str, list[str]]:
    title = f"Release 演练 {tag}（不发布）"
    runs = gh_api(
        f"repos/{repo()}/actions/workflows/release.yml/runs"
        f"?head_sha={sha}&event=workflow_dispatch&status=completed&per_page=50"
    )["workflow_runs"]
    runs = [r for r in runs if r.get("display_title") == title]
    runs.sort(key=lambda r: r["created_at"], reverse=True)
    wanted = [c for c in expected if c in REUSABLE]
    for run in runs:
        artifacts = gh_api(f"repos/{repo()}/actions/runs/{run['id']}/artifacts?per_page=100")["artifacts"]
        names = {a["name"] for a in artifacts if not a["expired"]}
        reuse = [
            c
            for c in wanted
            if f"rehearsal-sums-{c}" in names and (c != "server" or "docker-digest" in names)
        ]
        if reuse:
            print(f"Reusing rehearsal {run['html_url']} for: {', '.join(reuse)}")
            missing = [c for c in wanted if c not in reuse]
            if missing:
                print(f"::warning::演练未完成的组件将在本次重新构建：{', '.join(missing)}")
            return str(run["id"]), reuse
    print(f"No reusable rehearsal for {tag} @ {sha}; building from source")
    return "", []


def main() -> int:
    parser = argparse.ArgumentParser()
    sub = parser.add_subparsers(dest="cmd", required=True)
    gate = sub.add_parser("ci-gate")
    gate.add_argument("--sha", required=True)
    gate.add_argument("--timeout-min", type=int, default=60)
    find = sub.add_parser("find-rehearsal")
    find.add_argument("--tag", required=True)
    find.add_argument("--sha", required=True)
    find.add_argument("--expected", default="")
    find.add_argument("--github-output", action="store_true")
    args = parser.parse_args()

    if args.cmd == "ci-gate":
        return ci_gate(args.sha, args.timeout_min)

    expected = [c for c in args.expected.split(",") if c]
    run_id, reuse = find_rehearsal(args.tag, args.sha, expected)
    lines = [f"reuse_run={run_id}", f"reuse={','.join(reuse)}"]
    if args.github_output:
        with open(os.environ["GITHUB_OUTPUT"], "a", encoding="utf-8") as output:
            output.write("\n".join(lines) + "\n")
    else:
        print("\n".join(lines))
    return 0


if __name__ == "__main__":
    sys.exit(main())
