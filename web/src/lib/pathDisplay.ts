// 路径的中间省略显示：拆成「前段」与「末段（含前导分隔符）」，前段超宽时省略、末段尽量保留，
// 兼容 POSIX（`/`）与 Windows（`\`、盘符、UNC）路径；只用于显示，不改动实际路径。

/** 末段 = 最后一个分隔符起到结尾；去掉结尾多余分隔符（根目录本身除外）。没有可拆的分隔符时整段作末段。 */
export function splitPathTail(path: string): { head: string; tail: string } {
  let end = path.length
  while (end > 1 && (path[end - 1] === '/' || path[end - 1] === '\\') && !/^[A-Za-z]:[\\/]$/.test(path.slice(0, end))) end--
  const trimmed = path.slice(0, end)
  const cut = Math.max(trimmed.lastIndexOf('/'), trimmed.lastIndexOf('\\'))
  if (cut <= 0 || cut === trimmed.length - 1) return { head: '', tail: trimmed }
  return { head: trimmed.slice(0, cut), tail: trimmed.slice(cut) }
}
