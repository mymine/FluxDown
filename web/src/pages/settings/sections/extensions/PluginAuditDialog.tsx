// 插件审计日志对话框：按插件筛选、beforeId 翻页、二次确认清空、复制 JSON。动作码原样显示不翻译。

import { ScrollText } from 'lucide-react'
import { useCallback, useEffect, useRef, useState } from 'react'
import { useT } from '../../../../i18n'
import { copyText } from '../../../../lib/copy'
import { rpc, useDaemon } from '../../../../lib/rpc'
import type { DaemonSnapshot, PluginAuditEntryDto, PluginDto } from '../../../../lib/rpc'
import { Button, ConfirmFooter, Dialog, EmptyState, Select, toast } from '../../../../ui'
import { extensionErrorText } from './errors'

const EMPTY_PLUGINS: PluginDto[] = []
const selectPlugins = (daemon: DaemonSnapshot) => daemon.plugins
/** Select 不接受空字符串值：用哨兵表示「全部插件」。 */
const ALL = '*'
const PAGE_SIZE = 200

const OUTCOME_CLASS: Record<string, string> = {
  ok: 'text-foreground',
  denied: 'text-warning',
  error: 'text-destructive',
}

/** `initialIdentity`：打开时预选的插件（null = 全部）。`open=false` 时不请求。 */
export function PluginAuditDialog({ open, initialIdentity, onClose }: { open: boolean; initialIdentity: string | null; onClose: () => void }) {
  const t = useT()
  const plugins = useDaemon(selectPlugins, EMPTY_PLUGINS)
  const [filter, setFilter] = useState(initialIdentity ?? ALL)
  const [entries, setEntries] = useState<PluginAuditEntryDto[]>([])
  const [nextBeforeId, setNextBeforeId] = useState<number | undefined>(undefined)
  const [loading, setLoading] = useState(false)
  const [confirmClear, setConfirmClear] = useState(false)
  const [copied, setCopied] = useState(false)
  // 丢弃筛选切换前发出的过期响应。
  const generation = useRef(0)

  useEffect(() => {
    if (open) {
      setFilter(initialIdentity ?? ALL)
      setConfirmClear(false)
      setCopied(false)
    }
  }, [open, initialIdentity])

  const load = useCallback(
    async (beforeId: number | undefined) => {
      const ticket = ++generation.current
      setLoading(true)
      try {
        const page = await rpc.daemon.plugin.auditList({
          identity: filter === ALL ? undefined : filter,
          beforeId,
          limit: PAGE_SIZE,
        })
        if (ticket !== generation.current) return
        setEntries((current) => (beforeId === undefined ? page.entries : [...current, ...page.entries]))
        setNextBeforeId(page.nextBeforeId)
      } catch (error) {
        if (ticket === generation.current) toast.error(extensionErrorText(t, error))
      } finally {
        if (ticket === generation.current) setLoading(false)
      }
    },
    [filter, t],
  )

  useEffect(() => {
    if (!open) return
    setEntries([])
    setNextBeforeId(undefined)
    void load(undefined)
  }, [open, load])

  const clear = async () => {
    if (!confirmClear) {
      setConfirmClear(true)
      return
    }
    setConfirmClear(false)
    try {
      await rpc.daemon.plugin.auditClear({ identity: filter === ALL ? undefined : filter })
      await load(undefined)
    } catch (error) {
      toast.error(extensionErrorText(t, error))
    }
  }

  const copy = () => {
    copyText(JSON.stringify(entries, null, 2))
    setCopied(true)
    window.setTimeout(() => setCopied(false), 1500)
  }

  const options = [
    { value: ALL, label: t('pluginAuditFilterAll') },
    ...plugins.map((plugin) => ({ value: plugin.identity, label: plugin.name })),
  ]
  const columns = ['pluginAuditColTime', 'pluginAuditColOrigin', 'pluginAuditColAction', 'pluginAuditColTarget', 'pluginAuditColOutcome', 'pluginAuditColDetail']

  return (
    <Dialog
      open={open}
      onOpenChange={(next) => !next && onClose()}
      title={t('pluginAuditTitle')}
      size="xl"
      footer={
        <ConfirmFooter
          cancelLabel={null}
          okLabel={t('close')}
          onCancel={onClose}
          onOk={onClose}
          extra={
            <div className="flex gap-2">
              <Button variant={confirmClear ? 'danger' : 'outline'} disabled={entries.length === 0} onClick={() => void clear()}>
                {confirmClear ? t('pluginAuditClearConfirm') : t('pluginAuditClear')}
              </Button>
              <Button variant="outline" disabled={entries.length === 0} onClick={copy}>
                {copied ? t('pluginAuditCopied') : t('pluginAuditCopyJson')}
              </Button>
            </div>
          }
        />
      }
    >
      <div className="flex w-full flex-col gap-3 pb-2">
        <div className="max-w-xs">
          <Select value={filter} onValueChange={setFilter} options={options} aria-label={t('pluginAuditFilterAll')} />
        </div>
        {entries.length === 0 && !loading ? (
          <EmptyState icon={ScrollText} title={t('pluginAuditEmpty')} />
        ) : (
          <div className="max-h-[50vh] w-full overflow-auto">
            <table className="w-full min-w-[720px] text-left text-xs">
              <thead className="sticky top-0 bg-surface text-muted-foreground">
                <tr>
                  {columns.map((key) => (
                    <th key={key} className="px-2 py-1 font-medium">
                      {t(key)}
                    </th>
                  ))}
                </tr>
              </thead>
              <tbody>
                {entries.map((entry) => (
                  <tr key={entry.id} className="border-t border-hairline align-top">
                    <td className="tabular whitespace-nowrap px-2 py-1">{new Date(entry.atMs).toLocaleString()}</td>
                    <td className="px-2 py-1">{entry.origin}</td>
                    <td className="px-2 py-1 font-mono">{entry.action}</td>
                    <td className="break-all px-2 py-1 font-mono">{entry.target}</td>
                    <td className={`px-2 py-1 ${OUTCOME_CLASS[entry.outcome] ?? ''}`}>{entry.outcome}</td>
                    <td className="break-all px-2 py-1">{entry.detail}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
        {nextBeforeId !== undefined ? (
          <div className="flex justify-center">
            <Button variant="ghost" loading={loading} onClick={() => void load(nextBeforeId)}>
              {t('pluginAuditLoadMore')}
            </Button>
          </div>
        ) : null}
      </div>
    </Dialog>
  )
}
