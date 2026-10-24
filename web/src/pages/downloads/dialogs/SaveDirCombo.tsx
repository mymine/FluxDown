// 保存目录组合框（GPUI `pages/new_download.rs::render_save_dir_combo`）：照常手输，按下输入框即在其
// 正下方展开与输入框等宽的最近目录列表；点外部 / Esc / 选中收起。没有可选目录时就是普通输入框。
// 列表留在对话框内容里（不 portal）：模态对话框之外的节点不接收指针事件。

import { ChevronDown } from 'lucide-react'
import { useEffect, useRef, useState } from 'react'
import { Icon, Input } from '../../../ui'

export function SaveDirCombo({
  id,
  value,
  placeholder,
  choices,
  label,
  onChange,
}: {
  id: string
  value: string
  placeholder: string
  /** 可选目录（已排除当前输入）；空 = 不显示下拉。 */
  choices: readonly string[]
  /** 列表的无障碍名称。 */
  label: string
  onChange: (value: string) => void
}) {
  const [open, setOpen] = useState(false)
  const rootRef = useRef<HTMLDivElement>(null)
  const listId = `${id}-choices`
  const shown = open && choices.length > 0

  useEffect(() => {
    if (!shown) return
    const onPointerDown = (event: PointerEvent) => {
      if (!rootRef.current?.contains(event.target as Node)) setOpen(false)
    }
    document.addEventListener('pointerdown', onPointerDown, true)
    return () => document.removeEventListener('pointerdown', onPointerDown, true)
  }, [shown])

  return (
    <div ref={rootRef} className="relative w-full min-w-0">
      <Input
        id={id}
        value={value}
        placeholder={placeholder}
        spellCheck={false}
        role={choices.length > 0 ? 'combobox' : undefined}
        aria-expanded={choices.length > 0 ? shown : undefined}
        aria-controls={shown ? listId : undefined}
        onChange={(event) => onChange(event.target.value)}
        onMouseDown={() => setOpen(true)}
        onKeyDown={(event) => {
          if (event.key === 'ArrowDown') {
            setOpen(true)
          } else if (event.key === 'Escape' && shown) {
            // 只收起列表，不让 Esc 冒泡关掉整个对话框。
            event.stopPropagation()
            setOpen(false)
          }
        }}
        trailing={
          choices.length > 0 ? (
            <span className="pointer-events-none flex size-6 items-center justify-center text-text-tertiary">
              <Icon icon={ChevronDown} />
            </span>
          ) : undefined
        }
      />
      {shown ? (
        <div
          id={listId}
          role="listbox"
          aria-label={label}
          className="animate-fx-pop absolute inset-x-0 top-full z-50 mt-1 max-h-80 overflow-y-auto rounded-md border border-hairline bg-surface p-1 shadow-md"
        >
          {choices.map((dir) => (
            <button
              key={dir}
              type="button"
              role="option"
              aria-selected={false}
              title={dir}
              className="flex min-h-control w-full cursor-pointer items-center truncate rounded-sm px-2 text-left text-sm text-foreground outline-none hover:bg-nav-hover focus-visible:bg-nav-hover"
              // 保持输入框焦点：选中后可直接继续编辑。
              onMouseDown={(event) => event.preventDefault()}
              onClick={() => {
                onChange(dir)
                setOpen(false)
              }}
            >
              <span className="truncate">{dir}</span>
            </button>
          ))}
        </div>
      ) : null}
    </div>
  )
}
