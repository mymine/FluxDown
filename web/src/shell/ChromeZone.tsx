// 外壳区域：按布局渲染某个 zone 的条目。
// 编辑态（自定义布局）：区域始终渲染并带虚线轮廓 + 最小落点；条目 HTML5 可拖动，自身点击 / 菜单 / 焦点全部禁用（`inert`）；
// 每个可隐藏条目右上角有眼睛按钮（隐藏 / 显示，固定条目没有）；已隐藏条目调暗显示。
// 仅当拖动项的族允许放入该区域时 dragover 才接受并高亮；落在条目上 → 放到该条目处，落在空白处 → 追加到区域末尾。

import { Eye, EyeOff } from 'lucide-react'
import { cn } from '../lib/cn'
import { useT } from '../i18n'
import { usePluginActivityEntries } from '../pages/plugins/usePluginContributions'
import type { TooltipSide } from '../pages/downloads/statusbar/widget'
import { ACTIVITY_ENTRIES } from './activity'
import { getChromeCatalog, WIDGET_LABEL_KEY } from './chromeCatalog'
import { isHidden, itemAllowedIn, zoneFamily } from './chromeLayout'
import type { ChromeFamily, ChromeZone } from './chromeLayout'
import { beginChromeDrag, endChromeDrag, hoverChromeDrop, leaveChromeDrop, moveChromeItem, setChromeItemHidden, useChromeEdit, useChromeLayout } from './chromeStore'
import { ChromeItem } from './ChromeItem'
import { useChromeZoneItems } from './useChromeZoneItems'

const DRAG_MIME = 'application/x-fluxdown-chrome-item'
/** 各族的气泡方向：标题栏在顶部向下、活动栏在左侧向右、状态栏在底部向上。 */
const SIDE: Record<ChromeFamily, TooltipSide> = { title: 'bottom', rail: 'right', status: 'top' }

export function ChromeZoneView({ zone, className, keepEmpty = false }: { zone: ChromeZone; className?: string; keepEmpty?: boolean }) {
  const t = useT()
  const items = useChromeZoneItems(zone)
  const layout = useChromeLayout()
  const pluginEntries = usePluginActivityEntries()
  const { editing, dragId, hover } = useChromeEdit()
  const family = zoneFamily(zone)
  const orientation = family === 'rail' ? 'rail' : 'horizontal'
  const side = SIDE[family]

  if (!editing) {
    // 标题栏的空区域直接不渲染（避免多出一格间距）；活动栏 / 状态栏需要保留容器才能维持 justify-between 的两端对齐。
    if (items.length === 0 && !keepEmpty) return null
    return (
      <div className={className}>
        {items.map((id) => (
          <ChromeItem key={id} id={id} orientation={orientation} side={side} />
        ))}
      </div>
    )
  }

  const catalog = getChromeCatalog()
  const dragSpec = catalog.find((spec) => spec.id === dragId)
  const accepts = dragSpec !== undefined && itemAllowedIn(dragSpec, zone)
  const zoneHover = hover?.zone === zone && hover.target === null
  const labelOf = (id: string) => {
    const plugin = pluginEntries.find((entry) => entry.id === id)
    if (plugin) return plugin.label
    const key = ACTIVITY_ENTRIES.find((entry) => entry.id === id)?.labelKey ?? WIDGET_LABEL_KEY[id]
    return key === undefined ? id : t(key)
  }

  return (
    <div
      className={cn(
        className,
        'min-h-8 min-w-8 rounded-sm p-0.5 outline-dashed outline-1 -outline-offset-1 outline-border transition-colors',
        accepts && 'outline-accent-text',
        zoneHover && 'bg-drop-target',
      )}
      onDragOver={(event) => {
        if (!accepts) return
        event.preventDefault()
        event.dataTransfer.dropEffect = 'move'
        hoverChromeDrop(zone, null)
      }}
      onDragLeave={(event) => {
        if (!event.currentTarget.contains(event.relatedTarget as Node | null)) leaveChromeDrop(zone)
      }}
      onDrop={(event) => {
        if (!accepts || dragId === null) return
        event.preventDefault()
        moveChromeItem(dragId, zone, null)
        endChromeDrag()
      }}
    >
      {items.map((id) => {
        const hidden = isHidden(layout, id)
        const hideable = catalog.some((spec) => spec.id === id && !spec.fixed)
        const toggleLabel = t(hidden ? 'chromeLayoutShowItem' : 'chromeLayoutHideItem')
        return (
          // 外层承接落点并定位眼睛按钮；按钮不在可拖动元素内，点击它不会启动拖动。
          <div
            key={id}
            className={cn(
              'relative rounded-sm',
              accepts && dragId !== id && hover?.zone === zone && hover.target === id && 'bg-drop-target ring-1 ring-accent-text',
            )}
            onDragOver={(event) => {
              if (!accepts) return
              event.preventDefault()
              event.stopPropagation()
              event.dataTransfer.dropEffect = 'move'
              hoverChromeDrop(zone, id)
            }}
            onDrop={(event) => {
              if (!accepts || dragId === null) return
              event.preventDefault()
              event.stopPropagation()
              moveChromeItem(dragId, zone, id)
              endChromeDrag()
            }}
          >
            <div
              draggable
              title={labelOf(id)}
              className={cn(
                'flex cursor-grab items-center justify-center rounded-sm active:cursor-grabbing',
                (dragId === id || hidden) && 'opacity-40',
              )}
              onDragStart={(event) => {
                event.dataTransfer.setData(DRAG_MIME, id)
                event.dataTransfer.effectAllowed = 'move'
                // 推迟到拖动图像生成之后再改样式，否则拖动图像会带上半透明态。
                setTimeout(() => beginChromeDrag(id), 0)
              }}
              onDragEnd={endChromeDrag}
            >
              <div inert className="pointer-events-none flex items-center">
                <ChromeItem id={id} orientation={orientation} side={side} preview />
              </div>
            </div>
            {hideable ? (
              <button
                type="button"
                title={toggleLabel}
                aria-label={toggleLabel}
                aria-pressed={hidden}
                className="absolute -right-0.5 -top-0.5 z-10 flex size-4 items-center justify-center rounded-full border border-hairline bg-chrome text-muted-foreground hover:text-foreground"
                onClick={() => setChromeItemHidden(id, !hidden)}
              >
                {hidden ? <EyeOff className="size-3" aria-hidden /> : <Eye className="size-3" aria-hidden />}
              </button>
            ) : null}
          </div>
        )
      })}
    </div>
  )
}
