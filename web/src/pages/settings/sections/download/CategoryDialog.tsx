// 分类新增 / 编辑对话框（crates/settings/src/sections/category_dialog.rs）：字段、校验与保存语义逐条对齐。

import { useState } from 'react'
import { useT } from '../../../../i18n'
import { cn } from '../../../../lib/cn'
import { CATEGORY_ICON_KEYS, categoryIconByKey } from '../../../../lib/category-icons'
import type { CustomCategoryDto } from '../../../../lib/rpc'
import {
  Button,
  Dialog,
  DialogFooter,
  FieldError,
  FormField,
  Form,
  Icon,
  Input,
  InputWithAction,
  SegmentedTabs,
  confirmDialog,
} from '../../../../ui'
import { DirPickerDialog } from '../download/DirPickerDialog'
import { parseExtensions, regexLooksValid, writeCategories } from './categoryModel'

type MatchMode = 'extension' | 'regex'

export function CategoryDialog({
  open,
  onOpenChange,
  existing,
  list,
  defaultDir,
}: {
  open: boolean
  onOpenChange: (open: boolean) => void
  /** 编辑目标；null = 新增。 */
  existing: CustomCategoryDto | null
  /** 当前完整分类列表（保存时在其上增改）。 */
  list: readonly CustomCategoryDto[]
  /** 目录选择器的起点（默认下载目录）。 */
  defaultDir: string
}) {
  // 打开时按 existing 初始化；父级用 key 强制重挂载，避免残留上次输入。
  const t = useT()
  const [name, setName] = useState(existing?.name ?? '')
  const [icon, setIcon] = useState(existing?.icon ?? 'file')
  const [mode, setMode] = useState<MatchMode>(existing?.matchMode === 'regex' ? 'regex' : 'extension')
  const [extensions, setExtensions] = useState(existing?.extensions.join(', ') ?? '')
  const [regex, setRegex] = useState(existing?.regexPattern ?? '')
  const [saveDir, setSaveDir] = useState(existing?.saveDir ?? '')
  const [error, setError] = useState<string | null>(null)
  const [picking, setPicking] = useState(false)

  const isBuiltin = existing?.isBuiltin === true
  const builtinType = isBuiltin ? existing?.builtinType : null
  /** `all` 完全锁定；`other` 用排除逻辑匹配——两者都不展示匹配规则区。 */
  const specialBuiltin = builtinType === 'all' || builtinType === 'other'
  const canDelete = existing !== null && !existing.isBuiltin

  const save = () => {
    const trimmedName = name.trim()
    if (trimmedName === '' && !isBuiltin) return setError(t('categoryNameRequired'))
    let nextExtensions = existing?.extensions ?? []
    let nextRegex = existing?.regexPattern ?? ''
    if (!specialBuiltin) {
      if (mode === 'extension') {
        nextExtensions = parseExtensions(extensions)
        if (nextExtensions.length === 0 && !isBuiltin) return setError(t('extensionsRequired'))
        nextRegex = ''
      } else {
        nextRegex = regex.trim()
        if (nextRegex !== '' && !regexLooksValid(nextRegex)) return setError(t('regexInvalid'))
        nextExtensions = []
      }
    }
    const fields = {
      name: trimmedName,
      icon,
      matchMode: mode,
      extensions: nextExtensions,
      regexPattern: nextRegex,
      saveDir: saveDir.trim(),
    }
    if (existing) {
      writeCategories(list.map((entry) => (entry.id === existing.id ? { ...existing, ...fields } : entry)))
    } else {
      writeCategories([
        ...list,
        { id: `custom_${Date.now()}`, ...fields, position: 999, visible: true, isBuiltin: false, builtinType: null },
      ])
    }
    onOpenChange(false)
  }

  const remove = async () => {
    if (!existing) return
    const ok = await confirmDialog({
      title: t('deleteCategory'),
      description: t('deleteCategoryConfirm'),
      intent: 'destructive',
      okLabel: t('deleteCategory'),
    })
    if (!ok) return
    writeCategories(list.filter((entry) => entry.id !== existing.id))
    onOpenChange(false)
  }

  return (
    <>
      <Dialog
        open={open}
        onOpenChange={onOpenChange}
        title={t(existing ? 'editCategory' : 'addCategory')}
        size="md"
        footer={
          <DialogFooter>
            {canDelete ? (
              <Button variant="ghost" className="mr-auto text-destructive" onClick={() => void remove()}>
                {t('deleteCategory')}
              </Button>
            ) : null}
            <Button variant="outline" onClick={() => onOpenChange(false)}>
              {t('cancel')}
            </Button>
            <Button variant="primary" onClick={save}>
              {t('confirm')}
            </Button>
          </DialogFooter>
        }
      >
        <Form className="pb-2">
          <FormField label={t('categoryName')} htmlFor="category-name">
            <Input id="category-name" value={name} placeholder={t('categoryNameHint')} onChange={(event) => setName(event.target.value)} autoFocus />
          </FormField>
          <FormField label={t('categoryIcon')}>
            <div className="flex flex-wrap gap-1.5" role="radiogroup" aria-label={t('categoryIcon')}>
              {CATEGORY_ICON_KEYS.map((key) => {
                const selected = icon === key
                return (
                  <button
                    key={key}
                    type="button"
                    role="radio"
                    aria-checked={selected}
                    aria-label={key}
                    onClick={() => setIcon(key)}
                    className={cn(
                      'inline-flex size-control items-center justify-center rounded-md border coarse:size-11',
                      selected ? 'border-primary bg-accent text-primary' : 'border-border text-muted-foreground hover:bg-row-hover',
                    )}
                  >
                    <Icon icon={categoryIconByKey(key)} size="lg" />
                  </button>
                )
              })}
            </div>
          </FormField>
          {specialBuiltin ? null : (
            <>
              <FormField label={t('matchMode')}>
                <SegmentedTabs
                  items={[
                    { value: 'extension', label: t('matchByExtension') },
                    { value: 'regex', label: t('matchByRegex') },
                  ]}
                  value={mode}
                  onValueChange={(next) => {
                    setMode(next)
                    setError(null)
                  }}
                  className="self-start"
                />
              </FormField>
              <FormField label={t(mode === 'extension' ? 'extensionsLabel' : 'regexLabel')} htmlFor="category-match">
                <Input
                  id="category-match"
                  value={mode === 'extension' ? extensions : regex}
                  placeholder={t(mode === 'extension' ? 'extensionsHint' : 'regexHint')}
                  onChange={(event) => (mode === 'extension' ? setExtensions(event.target.value) : setRegex(event.target.value))}
                />
              </FormField>
            </>
          )}
          {builtinType === 'all' ? null : (
            <FormField label={t('categorySaveDir')} hint={t('categorySaveDirDesc')} htmlFor="category-dir">
              <InputWithAction
                input={<Input id="category-dir" value={saveDir} placeholder={t('selectSaveDir')} onChange={(event) => setSaveDir(event.target.value)} />}
                action={
                  <div className="flex gap-2">
                    <Button variant="outline" onClick={() => setPicking(true)}>
                      {t('browse')}
                    </Button>
                    {saveDir.trim() !== '' ? (
                      <Button variant="ghost" onClick={() => setSaveDir('')}>
                        {t('restoreDefaultPath')}
                      </Button>
                    ) : null}
                  </div>
                }
              />
            </FormField>
          )}
          {error ? <FieldError>{error}</FieldError> : null}
        </Form>
      </Dialog>
      <DirPickerDialog
        open={picking}
        onOpenChange={setPicking}
        initialPath={saveDir.trim() || defaultDir}
        onPick={(path) => {
          setSaveDir(path)
          setPicking(false)
        }}
      />
    </>
  )
}
