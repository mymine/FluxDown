// 自定义分类分组（crates/settings/src/sections/categories.rs `list_item`）。
// 排序：GPUI 用拖拽抓手；Web 用上移 / 下移按钮（键盘 / 触屏都可用，文案 `categoryPriorityNote`）。

import { ArrowDown, ArrowUp } from 'lucide-react'
import { useMemo, useState } from 'react'
import { useT } from '../../../../i18n'
import { categoryIconByKey } from '../../../../lib/category-icons'
import type { CustomCategoryDto } from '../../../../lib/rpc'
import { Badge, Button, Icon, Tooltip, confirmDialog } from '../../../../ui'
import { SettingsCustomRow, SettingsSection, useDaemonValue, usePrefRaw } from '../../kit'
import { CategoryDialog } from './CategoryDialog'
import {
  CUSTOM_CATEGORIES_PREF_KEY,
  applyAutoDirs,
  builtinDefaults,
  categoriesFromPreference,
  displayName,
  reorderCategories,
  writeCategories,
} from './categoryModel'

interface EditorState {
  /** 递增：强制对话框重挂载以重置草稿。 */
  serial: number
  existing: CustomCategoryDto | null
}

export function CategoriesGroup({ disabled }: { disabled: boolean }) {
  const t = useT()
  const raw = usePrefRaw(CUSTOM_CATEGORIES_PREF_KEY)
  const list = useMemo(() => categoriesFromPreference(raw), [raw])
  const defaultDir = useDaemonValue('default_save_dir')
  const [editor, setEditor] = useState<EditorState | null>(null)
  const [serial, setSerial] = useState(0)

  const openEditor = (existing: CustomCategoryDto | null) => {
    setSerial(serial + 1)
    setEditor({ serial: serial + 1, existing })
  }
  const move = (index: number, delta: -1 | 1) => {
    const from = list[index]
    const to = list[index + delta]
    if (!from || !to) return
    const next = reorderCategories(list, from.id, to.id)
    if (next) writeCategories(next)
  }
  const reset = async () => {
    const ok = await confirmDialog({
      title: t('resetBuiltinCategories'),
      description: t('resetAllCategoriesConfirm'),
      intent: 'destructive',
      okLabel: t('resetBuiltinCategories'),
    })
    if (ok) writeCategories(builtinDefaults())
  }

  return (
    <SettingsSection title={t('customCategories')} subtitle={t('categoryPriorityNote')}>
      <SettingsCustomRow className="flex flex-col gap-1 px-2 py-2">
        {list.map((entry, index) => {
          const details =
            entry.matchMode === 'regex'
              ? `${t('regexLabel')}: ${entry.regexPattern}`
              : entry.extensions.map((ext) => `.${ext}`).join(', ')
          return (
            <div key={entry.id} className="flex flex-wrap items-center gap-x-2 gap-y-1 rounded-md px-2 py-1.5 hover:bg-row-hover">
              <Icon icon={categoryIconByKey(entry.icon)} size="lg" className="shrink-0 text-muted-foreground" />
              <div className="flex min-w-0 flex-1 basis-40 flex-col gap-0.5">
                <div className="flex items-center gap-2 text-sm text-foreground">
                  <span className="min-w-0 truncate">{displayName(t, entry)}</span>
                  <Badge>{t(entry.isBuiltin ? 'builtinCategory' : 'customCategory')}</Badge>
                </div>
                {details ? <div className="truncate text-xs text-muted-foreground">{details}</div> : null}
                {entry.saveDir ? <div className="truncate text-xs text-text-tertiary">{entry.saveDir}</div> : null}
              </div>
              <div className="flex shrink-0 items-center gap-1">
                <Tooltip content={t('moveUpAction')}>
                  <Button variant="ghost" iconOnly className="text-muted-foreground hover:text-foreground" aria-label={t('moveUpAction')} disabled={disabled || index === 0} onClick={() => move(index, -1)}>
                    <Icon icon={ArrowUp} size="md" />
                  </Button>
                </Tooltip>
                <Tooltip content={t('moveDownAction')}>
                  <Button variant="ghost" iconOnly className="text-muted-foreground hover:text-foreground" aria-label={t('moveDownAction')} disabled={disabled || index === list.length - 1} onClick={() => move(index, 1)}>
                    <Icon icon={ArrowDown} size="md" />
                  </Button>
                </Tooltip>
                <Button variant="outline" disabled={disabled} onClick={() => openEditor(entry)}>
                  {t('editCategory')}
                </Button>
                <Button
                  variant="ghost"
                  className="text-destructive"
                  disabled={disabled || entry.isBuiltin}
                  onClick={() => writeCategories(list.filter((item) => item.id !== entry.id))}
                >
                  {t('delete')}
                </Button>
              </div>
            </div>
          )
        })}
        <div className="flex flex-wrap justify-end gap-2 px-2 pt-2 narrow:[&>*]:flex-1">
          <Button
            variant="outline"
            disabled={disabled || defaultDir.trim() === ''}
            onClick={() => {
              const next = applyAutoDirs(list, defaultDir)
              if (next) writeCategories(next)
            }}
          >
            {t('autoCategoryDirs')}
          </Button>
          <Button variant="outline" disabled={disabled} onClick={() => void reset()}>
            {t('resetBuiltinCategories')}
          </Button>
          <Button variant="primary" disabled={disabled} onClick={() => openEditor(null)}>
            {t('addCategory')}
          </Button>
        </div>
      </SettingsCustomRow>
      {editor ? (
        <CategoryDialog
          key={editor.serial}
          open
          onOpenChange={(open) => {
            if (!open) setEditor(null)
          }}
          existing={editor.existing}
          list={list}
          defaultDir={defaultDir}
        />
      ) : null}
    </SettingsSection>
  )
}
