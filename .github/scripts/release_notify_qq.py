#!/usr/bin/env python3
"""发版公告推送到 QQ 群（FD会员专属群，经 TgBot 平台 bot 3 的 Webhook 流程「FluxDown 发版公告」）。

- 只发中文更新日志：取 `<!-- fluxdown:lang:zh -->` 到下一个语言标记之间的区块；没有中文区块
  （AI 改写失败回退到 git-cliff 英文提交列表）时只发标题与链接，不发英文日志。
- QQ 不渲染任何富文本，Markdown 在这里转成可读纯文本：`###` 分组 → 【分组】，列表 → •，
  去掉加粗 / 删除线 / 行内代码 / 代码围栏标记，链接 → 「文字 (网址)」，图片与 HTML 注释 / 标签删除。
- TgBot 把请求体 `text` 填进 act.send_message 的 HTML 模板（变量自动转义），QQ 端再转回纯文本，
  所以这里发出的就是群里看到的原文。

用法：
  gh release view <tag> --json isDraft,isPrerelease,name,url,body > release.json
  QQ_WEBHOOK_URL=https://tg-bot.zerx.dev/hooks/<token> TAG=<tag> python3 .github/scripts/release_notify_qq.py release.json
  加 --dry-run 只打印消息不发送（不需要 QQ_WEBHOOK_URL）。
"""

import json
import os
import re
import sys
import urllib.request

ZH_RE = re.compile(r"<!-- fluxdown:lang:zh -->(.*?)(?=<!-- fluxdown:lang:|\Z)", re.S)
# 公告正文上限（字符）：QQ 长消息会被折叠，超出按整行截断并指向 Release 页面。
MAX_NOTES = 2500


def inline(s: str) -> str:
    """Markdown 行内语法 → 纯文本；行内代码内容原样保留，不做其它替换。"""
    out = []
    for i, part in enumerate(re.split(r"(`+)(.+?)\1", s)):
        # re.split 带两个捕获组：每 3 段为 [普通文本, 反引号, 代码内容]
        kind = i % 3
        if kind == 1:
            continue
        if kind == 2:
            out.append(part)
            continue
        # 先把 \* 等转义字符换成私用区占位，避免被下面的加粗 / 斜体规则吃掉，最后还原
        p = re.sub(r"\\([\\`*_{}\[\]()#+\-.!~|<>])", lambda m: chr(0xE000 + ord(m[1])), part)
        p = re.sub(r"!\[[^\]]*\]\([^)]*\)", "", p)  # 图片
        p = re.sub(r"\[([^\]]+)\]\(([^)\s]+)[^)]*\)", lambda m: link(m[1], m[2]), p)
        p = re.sub(r"<(https?://[^>\s]+)>", r"\1", p)  # 自动链接
        p = re.sub(r"<!--.*?-->|</?[A-Za-z][^>]*>", "", p)  # HTML 注释 / 标签
        p = re.sub(r"(\*\*|__)(.+?)\1", r"\2", p)  # 加粗
        p = re.sub(r"~~(.+?)~~", r"\1", p)  # 删除线
        # 斜体只认 *x*：_x_ 会误伤 snake_case 标识符
        p = re.sub(r"(?<![\w*])\*(?!\s)(.+?)(?<!\s)\*(?![\w*])", r"\1", p)
        p = re.sub(r"[\uE000-\uE07F]", lambda m: chr(ord(m[0]) - 0xE000), p)
        out.append(p)
    return "".join(out).rstrip()


def link(text: str, url: str) -> str:
    return text if text == url else f"{text} ({url})"


def zh_notes(body: str) -> str:
    m = ZH_RE.search(body)
    if not m:
        return ""
    lines: list[str] = []
    in_fence = False
    for raw in re.sub(r"<!--.*?-->", "", m[1], flags=re.S).splitlines():
        line = raw.rstrip()
        if re.match(r"^\s*(```|~~~)", line):
            in_fence = not in_fence
            continue
        if in_fence:
            lines.append(line)
            continue
        if re.match(r"^## \d", line):  # 版本号标题与公告标题重复
            continue
        if re.match(r"^\s*([-*_])(\s*\1){2,}\s*$", line):  # 分隔线
            continue
        h = re.match(r"^#{1,6}\s+(.*?)\s*#*$", line)
        li = re.match(r"^(\s*)[-*+]\s+(?:\[[ xX]\]\s+)?(.*)", line)
        ol = re.match(r"^(\s*)(\d+)[.)]\s+(.*)", line)
        quote = re.match(r"^\s*>\s?(.*)", line)
        if h:
            lines += ["", f"【{inline(h[1])}】"]
        elif li:
            lines.append(f"{'  ' * (len(li[1]) // 2)}• {inline(li[2])}")
        elif ol:
            lines.append(f"{'  ' * (len(ol[1]) // 2)}{ol[2]}. {inline(ol[3])}")
        elif quote:
            lines.append(f"｜{inline(quote[1])}")
        else:
            lines.append(inline(line))
    return re.sub(r"\n{3,}", "\n\n", "\n".join(lines)).strip()


def truncate(notes: str) -> str:
    if len(notes) <= MAX_NOTES:
        return notes
    kept, size = [], 0
    for line in notes.splitlines():
        if size + len(line) + 1 > MAX_NOTES:
            break
        kept.append(line)
        size += len(line) + 1
    return "\n".join(kept).rstrip() + "\n…（完整更新日志见下方链接）"


def message(release: dict, tag: str) -> str:
    title = "🧪 FluxDown 预览版" if release.get("isPrerelease") else "🚀 FluxDown 新版本"
    text = f"{title} {tag}\n"
    name = (release.get("name") or "").strip()
    if name and name != tag:
        text += f"{name}\n"
    notes = truncate(zh_notes(release.get("body") or ""))
    text += f"\n{notes}\n\n" if notes else "\n"
    return text + f"完整说明 / 下载：{release['url']}"


def main() -> int:
    args = [a for a in sys.argv[1:] if a != "--dry-run"]
    dry_run = len(args) != len(sys.argv) - 1
    if len(args) != 1:
        print(__doc__, file=sys.stderr)
        return 2
    with open(args[0], encoding="utf-8") as f:
        release = json.load(f)
    if release.get("isDraft"):
        print("Release is still a draft; skipping QQ announcement")
        return 0
    text = message(release, os.environ["TAG"])
    if dry_run:
        print(text)
        return 0
    url = os.environ.get("QQ_WEBHOOK_URL", "")
    if not url:
        print("::warning::QQ_RELEASE_WEBHOOK_URL secret is not set; skipping QQ announcement")
        return 0
    req = urllib.request.Request(
        url,
        data=json.dumps({"text": text}, ensure_ascii=False).encode(),
        # Cloudflare 会以 1010 拒绝 Python-urllib 默认 UA
        headers={"Content-Type": "application/json", "User-Agent": "fluxdown-release-notify/1.0"},
    )
    # 202 {"runs": n}：TgBot 已异步启动 n 次流程运行（每个选中群一次）；0 说明群被停用或不在范围内
    with urllib.request.urlopen(req, timeout=30) as resp:
        runs = json.load(resp).get("runs", 0)
    print(f"QQ webhook accepted: runs={runs}")
    return 0 if runs > 0 else 1


if __name__ == "__main__":
    sys.exit(main())
