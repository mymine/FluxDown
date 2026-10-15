// 重命名文件 / 更换下载链接：单字段对话框，引擎稳定错误码映射为本地化文案。

import { useEffect, useState } from 'react'
import type { ReactNode } from 'react'
import { useT } from '../../../i18n'
import { RpcError, TASK_STATUS, rpc, useTask } from '../../../lib/rpc'
import { rpcErrorText } from '../../../lib/rpcErrorText'
import type { TaskDto } from '../../../lib/rpc'
import { ThreadsRiskCallout } from '../../../lib/ThreadsRiskCallout'
import { ConfirmFooter, Dialog, FieldError, FieldHint, Form, FormField, Input, toast } from '../../../ui'
import { resumeIfFailed } from '../model/actions'
import { parseTaskSegments } from '../model/taskEdit'
import { closeChangeSegments, closeChangeUrl, closeRename } from './store'
import { mapEngineError } from './utils'

/** 引擎稳定错误码 → 文案键（native/engine `rename_task` 契约，错误消息即错误码）。 */
const RENAME_ERRORS: Readonly<Record<string, string>> = {
  'invalid-name': 'renameErrInvalidName',
  'task-active': 'renameErrTaskActive',
  'bt-unsupported': 'renameErrBtUnsupported',
  'target-exists': 'renameErrTargetExists',
  'not-found': 'renameErrNotFound',
}

/** 同上，`change_task_url` 契约。 */
const CHANGE_URL_ERRORS: Readonly<Record<string, string>> = {
  'invalid-url': 'webChangeUrlErrInvalidUrl',
  'task-active': 'webChangeUrlErrTaskActive',
  'task-completed': 'webChangeUrlErrTaskCompleted',
  'bt-unsupported': 'webChangeUrlErrBtUnsupported',
  'protocol-unsupported': 'webChangeUrlErrProtocolUnsupported',
  'protocol-mismatch': 'webChangeUrlErrProtocolMismatch',
  'not-found': 'webChangeUrlErrNotFound',
}

interface TaskScopedProps {
  taskId: string
  onClose: () => void
  render: (task: TaskDto) => ReactNode
}

function SingleFieldDialog({
  title,
  label,
  placeholder,
  okLabel,
  initial,
  submit,
  errors,
  timeoutKey,
  onClose,
}: {
  title: string
  label: string
  placeholder: string
  okLabel: string
  initial: string
  submit: (value: string) => Promise<unknown>
  errors: Readonly<Record<string, string>>
  timeoutKey?: string
  onClose: () => void
}) {
  const t = useT()
  const [value, setValue] = useState(initial)
  const [error, setError] = useState('')
  const [pending, setPending] = useState(false)

  const confirm = async () => {
    const trimmed = value.trim()
    if (trimmed === '' || pending) return
    if (trimmed === initial) {
      onClose()
      return
    }
    setPending(true)
    setError('')
    try {
      await submit(trimmed)
      onClose()
    } catch (err) {
      const key = err instanceof RpcError && err.is('timeout') && timeoutKey ? timeoutKey : mapEngineError(err, errors)
      setError(key ? t(key) : rpcErrorText(err, t))
      setPending(false)
    }
  }

  return (
    <Dialog
      open
      onOpenChange={(open) => !open && onClose()}
      size="md"
      title={title}
      footer={<ConfirmFooter okLabel={okLabel} okDisabled={value.trim() === ''} loading={pending} onCancel={onClose} onOk={() => void confirm()} />}
    >
      <Form onSubmit={() => void confirm()}>
        <FormField label={label} htmlFor="task-edit-field">
          <Input
            id="task-edit-field"
            value={value}
            placeholder={placeholder}
            spellCheck={false}
            autoFocus
            autoComplete="off"
            onFocus={(event) => event.target.select()}
            onChange={(event) => setValue(event.target.value)}
          />
        </FormField>
        {error ? <FieldError className="break-all">{error}</FieldError> : null}
        {/* 让回车提交：隐藏的提交按钮 */}
        <button type="submit" className="hidden" tabIndex={-1} aria-hidden />
      </Form>
    </Dialog>
  )
}

export function RenameDialog({ taskId }: { taskId: string }) {
  return <TaskScoped taskId={taskId} onClose={closeRename} render={(task) => <RenameBody taskId={taskId} fileName={task.fileName} />} />
}

function RenameBody({ taskId, fileName }: { taskId: string; fileName: string }) {
  const t = useT()
  return (
    <SingleFieldDialog
      title={t('renameTaskTitle')}
      label={t('colFileName')}
      placeholder={t('renameTaskPlaceholder')}
      okLabel={t('confirm')}
      initial={fileName}
      errors={RENAME_ERRORS}
      timeoutKey="renameTaskTimeout"
      onClose={closeRename}
      submit={async (name) => {
        await rpc.daemon.task.rename({ taskId, fileName: name })
        toast.key('renameTaskSuccess', 'success')
      }}
    />
  )
}

export function ChangeUrlDialog({ taskId }: { taskId: string }) {
  return (
    <TaskScoped
      taskId={taskId}
      onClose={closeChangeUrl}
      render={(task) => <ChangeUrlBody taskId={taskId} url={task.url} failed={task.status === TASK_STATUS.error} />}
    />
  )
}

/** 换链成功后失败任务自动重试（提交时刻的状态；暂停 / 等待保持原状态）。 */
function ChangeUrlBody({ taskId, url, failed }: { taskId: string; url: string; failed: boolean }) {
  const t = useT()
  return (
    <SingleFieldDialog
      title={t('webChangeUrlTitle')}
      label={t('webChangeUrlLabel')}
      placeholder={t('webChangeUrlPlaceholder')}
      okLabel={t('webChangeUrl')}
      initial={url}
      errors={CHANGE_URL_ERRORS}
      onClose={closeChangeUrl}
      submit={async (next) => {
        await rpc.daemon.task.changeUrl({ taskId, url: next })
        await resumeIfFailed(taskId, failed)
      }}
    />
  )
}

export function ChangeSegmentsDialog({ taskId }: { taskId: string }) {
  return (
    <TaskScoped
      taskId={taskId}
      onClose={closeChangeSegments}
      render={(task) => <ChangeSegmentsBody taskId={taskId} failed={task.status === TASK_STATUS.error} />}
    />
  )
}

/** 修改线程数：已下进度保留；成功后失败任务自动重试。任务 DTO 不含当前线程数，故输入框留空。 */
function ChangeSegmentsBody({ taskId, failed }: { taskId: string; failed: boolean }) {
  const t = useT()
  const [value, setValue] = useState('')
  const [error, setError] = useState('')
  const [pending, setPending] = useState(false)
  const segments = parseTaskSegments(value)

  const confirm = async () => {
    if (pending) return
    if (segments === null) {
      setError(t('editThreadsInvalid'))
      return
    }
    setPending(true)
    setError('')
    try {
      await rpc.daemon.task.setSegments({ taskId, segments })
    } catch (err) {
      // 已完成或已不存在的任务：daemon 统一返回 not-found。
      setError(err instanceof RpcError && err.is('notFound') ? t('threadsChangeRejected') : rpcErrorText(err, t))
      setPending(false)
      return
    }
    toast.key('threadsChanged', 'success')
    closeChangeSegments()
    await resumeIfFailed(taskId, failed)
  }

  return (
    <Dialog
      open
      onOpenChange={(open) => !open && closeChangeSegments()}
      size="md"
      title={t('editThreadsTitle')}
      footer={<ConfirmFooter okLabel={t('confirm')} okDisabled={value.trim() === ''} loading={pending} onCancel={closeChangeSegments} onOk={() => void confirm()} />}
    >
      <Form onSubmit={() => void confirm()}>
        <FormField label={t('configuredThreads')} htmlFor="task-segments-field" hint={t('queueDefaultSegmentsHint')}>
          <Input
            id="task-segments-field"
            value={value}
            placeholder="0"
            inputMode="numeric"
            autoFocus
            autoComplete="off"
            onChange={(event) => {
              setValue(event.target.value.replace(/\D/g, ''))
              setError('')
            }}
          />
        </FormField>
        <ThreadsRiskCallout segments={segments ?? 0} />
        <FieldHint>{t('editThreadsResetHint')}</FieldHint>
        {error ? <FieldError className="break-all">{error}</FieldError> : null}
        {/* 让回车提交：隐藏的提交按钮 */}
        <button type="submit" className="hidden" tabIndex={-1} aria-hidden />
      </Form>
    </Dialog>
  )
}

/** 任务被删除（快照里消失）时自动关闭；表单初值只在首次渲染时取。 */
function TaskScoped({ taskId, onClose, render }: TaskScopedProps) {
  const task = useTask(taskId)
  useEffect(() => {
    if (!task) onClose()
  }, [task, onClose])
  return task ? render(task) : null
}
