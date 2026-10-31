#!/usr/bin/env python3
"""清理 GitHub Actions artifact 与 tag 作用域缓存：两者都只是发布中间产物，正式存储在 GitHub Release。

artifact 保留规则（其余全部删除）：
  - 运行尚未结束（queued / in_progress）：并行的其他发布仍在使用自己的 artifact；
  - 运行名里带的版本 tag（「Release v1.2.3」「Release 演练 v1.2.3（不发布）」「Release 补发 v1.2.3 · app」
    「iOS TestFlight v1.2.3」）对应的 release 尚未正式发布（草稿或不存在）：演练待晋升、草稿待 Re-run 补齐。
一旦 release 转正，它的全部运行（tag 推送、演练、补发）的 artifact 在下一次清理时删除；
没被发布的演练由上传时的 retention-days 兜底过期，不会长期堆积。

缓存：GitHub 缓存按 ref 隔离，tag 作用域的缓存只有同一 tag 的 Re-run 能恢复（发布类 workflow 在 tag 上
已不保存缓存，见 release.yml 头部）。tag 的 release 已正式发布即删除，草稿 / 未建的保留给 Re-run。
API 返回的 ref 形如 `refs/tags/v1.2.3`，也见过 `refs/heads/refs/tags/v1.2.3`，两种都按 tag 处理。

用法：GH_TOKEN=… GH_REPO=owner/name python3 .github/scripts/release_artifacts.py [--dry-run]
"""

import json
import os
import re
import subprocess
import sys
from collections.abc import Callable

TAG_RE = re.compile(r"\bv\d+\.\d+\.\d+(?:-[0-9A-Za-z.]+)?")
TAG_REF_RE = re.compile(r"(?:^|/)refs/tags/(.+)$")


def gh(*args: str) -> str:
    return subprocess.run(["gh", *args], check=True, capture_output=True, text=True).stdout


def list_all(path: str, field: str) -> list[dict]:
    """分页列出 `GET path` 响应里 `field` 数组的全部元素。"""
    out = gh("api", "--paginate", f"{path}?per_page=100", "-q", f".{field}[] | @json")
    return [json.loads(line) for line in out.splitlines() if line]


def delete(repo: str, kind: str, item_id: int, dry_run: bool) -> bool:
    if dry_run:
        return True
    try:
        gh("api", "-X", "DELETE", f"repos/{repo}/actions/{kind}/{item_id}")
        return True
    except subprocess.CalledProcessError as err:
        print(f"::warning::failed to delete {kind} {item_id}: {err.stderr.strip()}")
        return False


def cleanup_artifacts(repo: str, is_published: Callable[[str], bool], dry_run: bool) -> None:
    runs: dict[int, dict] = {}

    def run_of(run_id: int) -> dict:
        if run_id not in runs:
            runs[run_id] = json.loads(gh("api", f"repos/{repo}/actions/runs/{run_id}"))
        return runs[run_id]

    deleted = kept = failed = 0
    for artifact in list_all(f"repos/{repo}/actions/artifacts", "artifacts"):
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
        if delete(repo, "artifacts", artifact["id"], dry_run):
            deleted += 1
        else:
            failed += 1
    print(f"Artifacts: deleted {deleted}, kept {kept}, failed {failed}{' (dry run)' if dry_run else ''}")


def cleanup_tag_caches(repo: str, is_published: Callable[[str], bool], dry_run: bool) -> None:
    deleted = kept = failed = 0
    for cache in list_all(f"repos/{repo}/actions/caches", "actions_caches"):
        ref_match = TAG_REF_RE.search(cache.get("ref") or "")
        if not ref_match:
            continue
        tag = ref_match.group(1)
        size_mb = cache["size_in_bytes"] / 1048576
        if not is_published(tag):
            kept += 1
            print(f"keep   cache {cache['key']} ({tag}, {size_mb:.1f} MB): {tag} not published yet")
            continue
        print(f"delete cache {cache['key']} ({tag}, {size_mb:.1f} MB)")
        if delete(repo, "caches", cache["id"], dry_run):
            deleted += 1
        else:
            failed += 1
    print(f"Tag caches: deleted {deleted}, kept {kept}, failed {failed}{' (dry run)' if dry_run else ''}")


def main() -> int:
    dry_run = "--dry-run" in sys.argv[1:]
    repo = os.environ["GH_REPO"]
    published: dict[str, bool] = {}

    def is_published(tag: str) -> bool:
        if tag not in published:
            try:
                info = json.loads(gh("release", "view", tag, "--repo", repo, "--json", "isDraft"))
                published[tag] = not info["isDraft"]
            except subprocess.CalledProcessError:
                published[tag] = False
        return published[tag]

    cleanup_artifacts(repo, is_published, dry_run)
    cleanup_tag_caches(repo, is_published, dry_run)
    return 0


if __name__ == "__main__":
    sys.exit(main())
