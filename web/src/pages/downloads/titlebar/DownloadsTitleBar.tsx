// 下载页挂到 shell 顶栏插槽的内容（GPUI title_bar.rs `DownloadTitleBar`）：
// 「新建下载」图标按钮（GPUI `toolbar_action_button`：ghost + 悬停提示）、搜索框、「视图」选项菜单；移动端在最左多一个打开侧栏抽屉的按钮。
// 调用方用 `<TitleBarSlot>` 包裹；这里只输出 flex 行的子元素（items-center、min-w-0）。

import { useState } from 'react'
import { PanelLeft, Plus, SlidersHorizontal } from 'lucide-react'
import { useT } from '../../../i18n'
import { Button, Popover, Tooltip, useIsMobile } from '../../../ui'
import { openNewDownload } from '../dialogs'
import { useDownloads } from '../state'
import { SearchBox } from './SearchBox'
import { ViewMenu } from './ViewMenu'
import type { ViewMenuPage } from './ViewMenu'

export function DownloadsTitleBar() {
  const t = useT()
  const mobile = useIsMobile()
  const { sidebarSelection, setSidebarOpen } = useDownloads()
  // 「视图」弹层当前分页（关闭再打开保持，与 GPUI 相同）。
  const [page, setPage] = useState<ViewMenuPage>('columns')

  return (
    <>
      {mobile ? (
        <Button
          variant="ghost"
          iconOnly
          icon={PanelLeft}
          title={t('webOpenSidebar')}
          aria-label={t('webOpenSidebar')}
          onClick={() => setSidebarOpen(true)}
        />
      ) : null}
      <Tooltip content={t('newDownload')}>
        <Button
          variant="ghost"
          iconOnly
          icon={Plus}
          aria-label={t('newDownload')}
          onClick={() =>
            // GPUI `build_new_download_context` 优先取侧栏选中的队列。
            openNewDownload(sidebarSelection.kind === 'queue' ? { queueId: sidebarSelection.queueId } : undefined)
          }
        />
      </Tooltip>
      <div className="min-w-0 flex-1 mobile:hidden" />
      <SearchBox />
      <Popover
        align="end"
        title={t('viewOptionsTitle')}
        className="w-[320px]"
        trigger={
          <Tooltip content={t('viewOptionsTitle')}>
            <Button variant="ghost" icon={SlidersHorizontal} iconOnly={mobile} aria-label={t('viewMenuLabel')}>
              {mobile ? null : t('viewMenuLabel')}
            </Button>
          </Tooltip>
        }
      >
        <ViewMenu page={page} onPageChange={setPage} />
      </Popover>
    </>
  )
}
