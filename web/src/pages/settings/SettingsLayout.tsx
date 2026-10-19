// 设置：Web 里是整页（没有独立窗口）。桌面 = 左侧 200px 分类导航 + 内容；
// 移动端 = `/settings` 显示分类列表，进入分类后整屏内容 + 标题栏返回键。
// 分类内容见 categories.ts；此文件只负责布局与导航，不含具体设置项。

import { Link, Navigate, Outlet, useLocation, useNavigate, useParams } from '@tanstack/react-router'
import { ChevronLeft, ChevronRight } from 'lucide-react'
import { useT } from '../../i18n'
import { cn } from '../../lib/cn'
import { TitleBarSlot } from '../../shell'
import { Button, Icon, useIsMobile } from '../../ui'
import { findSettingsCategory } from './categories'
import { useVisibleCategories } from './useVisibleCategories'

function CategoryList({ activeId }: { activeId: string | null }) {
  const t = useT()
  const { categories } = useVisibleCategories()
  return (
    <ul className="flex flex-col gap-0.5 p-2">
      {categories.map((category) => {
        const active = category.id === activeId
        return (
          <li key={category.id}>
            <Link
              to="/settings/$category"
              params={{ category: category.id }}
              aria-current={active ? 'page' : undefined}
              className={cn(
                'flex h-nav-row items-center gap-2 rounded-[var(--fx-components-nav-item-radius)] px-2 text-sm text-foreground transition-colors hover:bg-nav-hover mobile:min-h-touch mobile:gap-3 mobile:px-3',
                active && 'bg-nav-selected',
              )}
            >
              <Icon icon={category.icon} className={cn('text-muted-foreground', active && 'text-nav-selected-icon')} />
              <span className="min-w-0 flex-1 truncate">{t(category.labelKey)}</span>
              <Icon icon={ChevronRight} className="text-text-tertiary desktop:hidden" />
            </Link>
          </li>
        )
      })}
    </ul>
  )
}

/** `/settings` 索引：桌面跳到第一个分类，移动端显示分类列表。 */
export function SettingsIndex() {
  const mobile = useIsMobile()
  const { categories } = useVisibleCategories()
  if (!mobile) return <Navigate to="/settings/$category" params={{ category: categories[0]?.id ?? 'general' }} replace />
  return (
    <div className="h-full overflow-y-auto">
      <CategoryList activeId={null} />
    </div>
  )
}

export function SettingsLayout() {
  const t = useT()
  const mobile = useIsMobile()
  const navigate = useNavigate()
  const { pathname } = useLocation()
  const activeId = pathname.startsWith('/settings/') ? (pathname.split('/')[2] ?? null) : null
  const active = activeId ? findSettingsCategory(activeId) : undefined

  return (
    <div className="flex h-full min-h-0">
      <TitleBarSlot>
        {mobile && active ? (
          <>
            <Button variant="ghost" iconOnly aria-label={t('back')} onClick={() => navigate({ to: '/settings' })}>
              <Icon icon={ChevronLeft} />
            </Button>
            <span className="min-w-0 flex-1 truncate text-sm font-medium text-foreground">{t(active.labelKey)}</span>
          </>
        ) : (
          <span className="min-w-0 flex-1 truncate text-sm font-medium text-foreground">{t('settings')}</span>
        )}
      </TitleBarSlot>
      {mobile ? null : (
        <aside className="w-[200px] shrink-0 overflow-y-auto border-r border-hairline bg-chrome">
          <CategoryList activeId={activeId} />
        </aside>
      )}
      <div className="min-w-0 flex-1 overflow-y-auto">
        <Outlet />
      </div>
    </div>
  )
}

/** `/settings/$category`：渲染注册表里对应分类的组件；条件分类不满足时（含当前就在该页而条件失效）退回账户页。 */
export function SettingsCategoryRoute() {
  const { category = '' } = useParams({ strict: false })
  const { ready, categories } = useVisibleCategories()
  const entry = findSettingsCategory(category)
  if (!entry) return <Navigate to="/settings" replace />
  if (!categories.includes(entry)) return ready ? <Navigate to="/settings/$category" params={{ category: 'account' }} replace /> : null
  const Content = entry.Component
  return <Content />
}
