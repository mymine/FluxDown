#!/usr/bin/env python3
"""清理 GitHub Actions artifact：artifact 只是发布中间产物，正式存储在 GitHub Release。

保留规则（其余全部删除）：
  - 运行尚未结束（queued / in_progress）：并行的其他发布仍在使用自己的 artifact；
  - 运行名里带的版本 tag（「Release v1.2.3」「Release 演练 v1.2.3（不发布）」「Release 补发 v1.2.3 · app」
    「iOS TestFlight v1.2.3」）对应的 release 尚未正式发布（草稿或不存在）：演练待晋升、草稿待 Re-run 补齐。
一旦 release 转正，它的全部运行（tag 推送、演练、补发）的 artifact 在下一次清理时删除；
没被发布的演练由上传时的 retention-days 兜底过期，不会长期堆积。

用法：GH_TOKEN=… GH_REPO=owner/name python3 .github/scripts/release_artifacts.py [--dry-run]
"""

import json
import os
import re
import subprocess
import sys

TAG_RE = re.compile(r"\bv\d+\.\d+\.\d+(?:-[0-9A-Za-z.]+)?")


def gh(*args: str) -> str:
    return subprocess.run(["gh", *args], check=True, capture_output=True, text=True).stdout


def main() -> int:
    dry_run = "--dry-run" in sys.argv[1:]
    repo = os.environ["GH_REPO"]
    artifacts = [
        json.loads(line)
        for line in gh(
            "api", "--paginate", f"repos/{repo}/actions/artifacts?per_page=100", "-q", ".artifacts[] | @json"
        ).splitlines()
        if line
    ]
    runs: dict[int, dict] = {}
    published: dict[str, bool] = {}

    def run_of(run_id: int) -> dict:
        if run_id not in runs:
            runs[run_id] = json.loads(gh("api", f"repos/{repo}/actions/runs/{run_id}"))
        return runs[run_id]

    def is_published(tag: str) -> bool:
        if tag not in published:
            try:
                info = json.loads(gh("release", "view", tag, "--repo", repo, "--json", "isDraft"))
                published[tag] = not info["isDraft"]
            except subprocess.CalledProcessError:
                published[tag] = False
        return published[tag]

    deleted = kept = failed = 0
    for artifact in artifacts:
        if artifact["expired"]:
            continue
        run_id = artifact["workflow_run"]["id"]
        try:
            run = run_of(run_id)
        except subprocess.CalledProcessError:
            run = {"status": "completed", "display_title": ""}
        match = TAG_RE.search(run.get("display_title") or "")
        reason = None
        if run["status"] != "completed":
            reason = "run in progress"
        elif match and not is_published(match.group(0)):
            reason = f"{match.group(0)} not published yet"
        size_mb = artifact["size_in_bytes"] / 1048576
        if reason:
            kept += 1
            print(f"keep   {artifact['name']} (run {run_id}, {size_mb:.1f} MB): {reason}")
            continue
        print(f"delete {artifact['name']} (run {run_id}, {size_mb:.1f} MB)")
        if dry_run:
            deleted += 1
            continue
        try:
            gh("api", "-X", "DELETE", f"repos/{repo}/actions/artifacts/{artifact['id']}")
            deleted += 1
        except subprocess.CalledProcessError as err:
            failed += 1
            print(f"::warning::failed to delete artifact {artifact['id']}: {err.stderr.strip()}")
    print(f"Artifacts: deleted {deleted}, kept {kept}, failed {failed}{' (dry run)' if dry_run else ''}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
