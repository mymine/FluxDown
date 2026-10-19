// 轻量 markdown：只分「围栏代码块」与「段落」（空行分段，段内换行保留），不引入 Markdown 渲染依赖。

export type MarkdownBlock = { type: 'paragraph'; text: string } | { type: 'code'; text: string; lang: string }

export function splitMarkdown(source: string): MarkdownBlock[] {
  const blocks: MarkdownBlock[] = []
  let paragraph: string[] = []
  let code: string[] | null = null
  let lang = ''
  const flush = () => {
    if (paragraph.length > 0) blocks.push({ type: 'paragraph', text: paragraph.join('\n') })
    paragraph = []
  }
  for (const line of source.replace(/\r\n?/g, '\n').split('\n')) {
    const fence = /^\s*```(.*)$/.exec(line)
    if (code !== null) {
      if (fence && fence[1]?.trim() === '') {
        blocks.push({ type: 'code', text: code.join('\n'), lang })
        code = null
      } else {
        code.push(line)
      }
    } else if (fence) {
      flush()
      code = []
      lang = fence[1]?.trim() ?? ''
    } else if (line.trim() === '') {
      flush()
    } else {
      paragraph.push(line)
    }
  }
  if (code !== null) blocks.push({ type: 'code', text: code.join('\n'), lang })
  flush()
  return blocks
}
