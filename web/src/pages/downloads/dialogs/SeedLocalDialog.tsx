// 本机做种：把守护进程所在机器上已有的文件 / 目录登记为已完成 BT 任务并开始做种。
// Web 无服务器文件选择器（服务端文件系统 ≠ 浏览器文件系统），路径手动输入。
// 慢请求（哈希大数据可达数分钟）：提交期间忙碌态、不可关闭；创建类提交不自动重试。

import { useState } from 'react'
import { useT } from '../../../i18n'
import { rpc } from '../../../lib/rpc'
import { rpcErrorText } from '../../../lib/rpcErrorText'
import { ConfirmFooter, Dialog, FieldError, FieldHint, Form, FormField, Input, OptionGroup, OptionRow, Spinner, Switch, Textarea, toast } from '../../../ui'
import { buildSeedLocalParams, seedLocalFeedback } from './seedLocal'
import { closeSeedLocal } from './store'

export function SeedLocalDialog() {
  const t = useT()
  const [path, setPath] = useState('')
  const [torrentPath, setTorrentPath] = useState('')
  const [trackers, setTrackers] = useState('')
  const [unlimited, setUnlimited] = useState(false)
  const [error, setError] = useState('')
  const [pending, setPending] = useState(false)

  const submit = async () => {
    if (pending) return
    const built = buildSeedLocalParams({ path, torrentPath, trackers, unlimited })
    if (!built.ok) {
      setError(t('seedLocalErrPathRequired'))
      return
    }
    setPending(true)
    setError('')
    try {
      const result = await rpc.daemon.task.seedLocal(built.params)
      const fb = seedLocalFeedback(result)
      if ('params' in fb) toast.key(fb.key, fb.level, fb.params)
      else toast.key(fb.key, fb.level)
      closeSeedLocal()
    } catch (err) {
      setError(rpcErrorText(err, t))
      setPending(false)
    }
  }

  return (
    <Dialog
      open
      onOpenChange={(open) => !open && !pending && closeSeedLocal()}
      modalLocked={pending}
      size="md"
      title={t('seedLocalTitle')}
      description={t('seedLocalDesc')}
      footer={<ConfirmFooter okLabel={t('seedLocalSubmit')} loading={pending} okDisabled={pending} onCancel={closeSeedLocal} onOk={() => void submit()} />}
    >
      <Form onSubmit={() => void submit()}>
        <FormField label={t('seedLocalPath')} htmlFor="seed-local-path">
          <Input
            id="seed-local-path"
            value={path}
            placeholder={t('seedLocalPathPlaceholder')}
            spellCheck={false}
            autoFocus
            autoComplete="off"
            disabled={pending}
            onChange={(event) => setPath(event.target.value)}
            className="font-mono text-xs"
          />
        </FormField>
        <FormField label={t('seedLocalTorrent')} htmlFor="seed-local-torrent" hint={t('seedLocalTorrentHint')}>
          <Input
            id="seed-local-torrent"
            value={torrentPath}
            spellCheck={false}
            autoComplete="off"
            disabled={pending}
            onChange={(event) => setTorrentPath(event.target.value)}
            className="font-mono text-xs"
          />
        </FormField>
        {torrentPath.trim() === '' ? (
          <FormField label={t('seedLocalTrackers')} htmlFor="seed-local-trackers" hint={t('seedLocalTrackersHint')}>
            <Textarea
              id="seed-local-trackers"
              value={trackers}
              spellCheck={false}
              disabled={pending}
              onChange={(event) => setTrackers(event.target.value)}
              className="font-mono text-xs"
            />
          </FormField>
        ) : null}
        <OptionGroup>
          <OptionRow
            title={t('seedLocalUnlimited')}
            description={t('seedLocalUnlimitedDesc')}
            control={<Switch checked={unlimited} disabled={pending} onCheckedChange={setUnlimited} aria-label={t('seedLocalUnlimited')} />}
          />
        </OptionGroup>
        {pending ? (
          <FieldHint className="flex items-center gap-2">
            <Spinner />
            {t('seedLocalWorking')}
          </FieldHint>
        ) : null}
        {error ? <FieldError className="break-all">{error}</FieldError> : null}
        <button type="submit" className="hidden" tabIndex={-1} aria-hidden />
      </Form>
    </Dialog>
  )
}
