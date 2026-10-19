// 自定义布局的入口与编辑态提示：
// 标题栏 / 活动栏 / 状态栏右键 → 「自定义布局… / 恢复默认布局」；编辑态在标题栏下方显示一条横幅（提示 + 重置 + 完成）。

import { LayoutPanelLeft, RotateCcw } from 'lucide-react'
import type { MouseEvent, ReactNode } from 'react'
import { useT } from '../i18n'
import { Button, ContextMenuArea } from '../ui'
import { resetChromeLayout, setChromeEditing, useChromeEdit } from './chromeStore'

/** 弹层 / 对话框经 React 树冒泡会带着右键事件回到这里（如账户卡片）；它们保留自己的右键行为，不弹布局菜单。 */
function keepDialogContextMenu(event: MouseEvent<HTMLElement>): void {
  if ((event.target as HTMLElement).closest('[role="dialog"]')) event.stopPropagation()
}

/** 右键菜单区域：`contents` 不引入额外盒子，原有 flex 布局不变。编辑态关闭菜单（横幅里已有重置 / 完成）。 */
export function ChromeMenuArea({ children }: { children: ReactNode }) {
  const t = useT()
  const { editing } = useChromeEdit()
  return (
    <ContextMenuArea
      className="contents"
      title={t('chromeLayoutCustomize')}
      disabled={editing}
      entries={[
        { type: 'item', key: 'customize', label: t('chromeLayoutCustomize'), icon: LayoutPanelLeft, onSelect: () => setChromeEditing(true) },
        { type: 'item', key: 'reset', label: t('chromeLayoutReset'), icon: RotateCcw, onSelect: resetChromeLayout },
      ]}
    >
      <div className="contents" onContextMenu={keepDialogContextMenu}>
        {children}
      </div>
    </ContextMenuArea>
  )
}

export function ChromeEditBanner() {
  const t = useT()
  const { editing } = useChromeEdit()
  if (!editing) return null
  return (
    <div role="status" className="flex shrink-0 items-center gap-2 border-b border-hairline bg-nav-selected px-3 py-1 text-xs text-foreground">
      <span className="min-w-0 flex-1 truncate">{t('chromeLayoutHint')}</span>
      <Button icon={RotateCcw} className="h-6 min-h-0" onClick={resetChromeLayout}>
        {t('chromeLayoutReset')}
      </Button>
      <Button variant="primary" className="h-6 min-h-0" onClick={() => setChromeEditing(false)}>
        {t('chromeLayoutDone')}
      </Button>
    </div>
  )
}
