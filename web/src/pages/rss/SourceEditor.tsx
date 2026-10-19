// 订阅编辑器（GPUI `crates/rss/src/editor.rs`）：基本 / 过滤 / 高级三个 Tab，
// 新建须先验证订阅地址（验证结果绑定其请求参数，输入变化即失效），编辑保留只读运行态字段。
// 来源类型（内置 RSS / 插件 provider）与 provider 配置的规则见 `./provider.ts`。
// 桌面居中 Dialog，移动端全屏。

import { CheckCircle2, FolderOpen } from 'lucide-react'
import { useEffect, useMemo, useRef, useState } from 'react'
import { useT } from '../../i18n'
import { MAIN_QUEUE_ID, RpcError, errorMessage, rpc } from '../../lib/rpc'
import type { PluginDto, QueueDto, RssItemDto, RssSourceDto, RssSourceInput, RssValidateRequest } from '../../lib/rpc'
import {
  Button,
  Card,
  ConfirmFooter,
  Dialog,
  FieldError,
  FieldHint,
  Form,
  FormField,
  FormRow,
  InputWithAction,
  Input,
  OptionGroup,
  OptionRow,
  SegmentedTabs,
  Select,
  Switch,
  Icon,
} from '../../ui'
import { FsPickerDialog } from '../downloads/dialogs/FsPickerDialog'
import { formatSize, parseSize } from './filter'
import type { RssFilterRule } from './filter'
import { DEFAULT_INTERVAL_MINUTES, DEFAULT_MAX_PER_FETCH, effectiveInterval, intervalLabel, queueLabel } from './format'
import { FilterPreview } from './FilterPreview'
import { BUILTIN_PROVIDER_ID, normalizeProviderId, providerChoices, validateProviderConfig } from './provider'
import type { ProviderChoice } from './provider'

const INTERVALS = [10, 30, 60, 120, 360, 720, 1440]

type Tab = 'basic' | 'filter' | 'advanced'

interface EditorForm {
  providerId: string
  providerConfig: string
  name: string
  url: string
  saveDir: string
  include: string
  exclude: string
  sizeMin: string
  sizeMax: string
  cookies: string
  userAgent: string
  proxyUrl: string
  maxPerFetch: string
  interval: number
  queueId: string
  enabled: boolean
  autoDownload: boolean
  startPaused: boolean
  useRegex: boolean
  smartEpisode: boolean
  sendReferer: boolean
  notifyOnDownload: boolean
}

interface ValidatedFeed {
  request: RssValidateRequest
  title: string
  items: RssItemDto[]
}

function initialForm(source: RssSourceDto | null): EditorForm {
  return {
    providerId: normalizeProviderId(source?.providerId ?? ''),
    providerConfig: source?.providerConfig ?? '',
    name: source?.name ?? '',
    url: source?.url ?? '',
    saveDir: source?.saveDir ?? '',
    include: source?.includePattern ?? '',
    exclude: source?.excludePattern ?? '',
    sizeMin: source ? formatSize(source.sizeMinBytes) : '',
    sizeMax: source ? formatSize(source.sizeMaxBytes) : '',
    cookies: source?.cookies ?? '',
    userAgent: source?.userAgent ?? '',
    proxyUrl: source?.proxyUrl ?? '',
    maxPerFetch: String(source && source.maxPerFetch > 0 ? source.maxPerFetch : DEFAULT_MAX_PER_FETCH),
    interval: source ? effectiveInterval(source) : DEFAULT_INTERVAL_MINUTES,
    queueId: source && source.queueId !== '' ? source.queueId : MAIN_QUEUE_ID,
    enabled: source?.enabled ?? true,
    autoDownload: source?.autoDownload ?? true,
    startPaused: source?.startPaused ?? false,
    useRegex: source?.useRegex ?? false,
    smartEpisode: source?.smartEpisode ?? false,
    sendReferer: source?.sendReferer ?? true,
    notifyOnDownload: source?.notifyOnDownload ?? true,
  }
}

function requestOf(form: EditorForm): RssValidateRequest {
  return {
    providerId: form.providerId,
    providerConfig: form.providerId === BUILTIN_PROVIDER_ID ? '' : form.providerConfig.trim(),
    url: form.url.trim(),
    cookies: form.cookies.trim(),
    userAgent: form.userAgent.trim(),
    proxyUrl: form.proxyUrl.trim(),
  }
}

function sameRequest(a: RssValidateRequest, b: RssValidateRequest): boolean {
  return (
    a.providerId === b.providerId &&
    a.providerConfig === b.providerConfig &&
    a.url === b.url &&
    a.cookies === b.cookies &&
    a.userAgent === b.userAgent &&
    a.proxyUrl === b.proxyUrl
  )
}

/** 空体积 = 不限（0）；非法返回 null。 */
function sizeField(text: string): number | null {
  return text.trim() === '' ? 0 : parseSize(text)
}

/** 抓取上限：1..=100 的整数。 */
function fetchLimit(text: string): number | null {
  const trimmed = text.trim()
  if (!/^\d+$/.test(trimmed)) return null
  const value = Number(trimmed)
  return value >= 1 && value <= 100 ? value : null
}

function rpcErrorText(error: unknown): string {
  if (error instanceof RpcError && error.appCode) return error.field ? `${error.appCode}: ${error.field}` : error.appCode
  return errorMessage(error)
}

export function SourceEditor({
  open,
  source,
  queues,
  plugins,
  onOpenChange,
}: {
  open: boolean
  /** null = 新建。 */
  source: RssSourceDto | null
  queues: readonly QueueDto[]
  /** 已安装插件：打开时取一次快照生成来源选项（与 GPUI 一致，编辑期间不随插件变化重排）。 */
  plugins: readonly PluginDto[]
  onOpenChange: (open: boolean) => void
}) {
  const t = useT()
  const [form, setForm] = useState<EditorForm>(() => initialForm(source))
  const [choices] = useState<ProviderChoice[]>(() => providerChoices(plugins, source?.providerId ?? ''))
  const [tab, setTab] = useState<Tab>('basic')
  const [validated, setValidated] = useState<ValidatedFeed | null>(null)
  const [validating, setValidating] = useState(false)
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [pickingDir, setPickingDir] = useState(false)
  const [cached, setCached] = useState<readonly RssItemDto[] | null>(null)
  const formRef = useRef(form)
  formRef.current = form

  const existing = source !== null
  const request = requestOf(form)
  const validOk = validated !== null && sameRequest(validated.request, request)

  // 既有订阅的过滤预览用已缓存条目；首次进入「过滤」Tab 时才拉取。
  const sourceId = source?.sourceId ?? null
  useEffect(() => {
    if (tab !== 'filter' || sourceId === null || cached !== null) return
    let cancelled = false
    rpc.daemon.rss.getItems({ sourceId }).then(
      (items) => {
        if (!cancelled) setCached(items)
      },
      () => {
        if (!cancelled) setCached([])
      },
    )
    return () => {
      cancelled = true
    }
  }, [tab, sourceId, cached])

  const patch = (changes: Partial<EditorForm>) => setForm((prev) => ({ ...prev, ...changes }))
  /** 会改变验证请求的字段：同时清掉旧错误。 */
  const patchRequest = (changes: Partial<EditorForm>) => {
    setError(null)
    patch(changes)
  }

  const rule = useMemo<RssFilterRule>(
    () => ({
      include: form.include,
      exclude: form.exclude,
      useRegex: form.useRegex,
      smartEpisode: form.smartEpisode,
      sizeMinBytes: sizeField(form.sizeMin) ?? 0,
      sizeMaxBytes: sizeField(form.sizeMax) ?? 0,
    }),
    [form.include, form.exclude, form.useRegex, form.smartEpisode, form.sizeMin, form.sizeMax],
  )
  const previewItems = useMemo<readonly RssItemDto[]>(() => {
    if (cached !== null && cached.length > 0) return cached
    return validated?.items ?? []
  }, [cached, validated])

  const validate = async () => {
    if (validating || saving) return
    const sent = requestOf(formRef.current)
    if (sent.url === '') {
      setTab('basic')
      setError(t('rssFeedRequired'))
      return
    }
    if (sent.providerId !== BUILTIN_PROVIDER_ID && validateProviderConfig(sent.providerConfig ?? '') === null) {
      setTab('basic')
      setError(t('rssProviderConfigInvalid'))
      return
    }
    setValidating(true)
    setValidated(null)
    setError(null)
    try {
      const response = await rpc.daemon.rss.validate(sent)
      // 编辑过程中即使旧请求晚到，也不接受与当前输入不同的结果。
      if (!sameRequest(requestOf(formRef.current), sent)) return
      if (response.error === '') {
        setValidated({ request: sent, title: response.feedTitle, items: response.items })
        setTab('basic')
      } else {
        setError(response.error)
      }
    } catch (err) {
      if (sameRequest(requestOf(formRef.current), sent)) setError(rpcErrorText(err))
    } finally {
      setValidating(false)
    }
  }

  const fail = (target: Tab, key: string) => {
    setTab(target)
    setError(t(key))
  }

  const save = async () => {
    if (saving || validating) return
    if (request.url === '') return fail('basic', 'rssFeedRequired')
    if (request.providerId !== BUILTIN_PROVIDER_ID && validateProviderConfig(request.providerConfig ?? '') === null) {
      return fail('basic', 'rssProviderConfigInvalid')
    }
    if (!existing && !validOk) return fail('basic', 'rssValidateBeforeSave')
    const min = sizeField(form.sizeMin)
    const max = sizeField(form.sizeMax)
    if (min === null || max === null) return fail('filter', 'rssInvalidNumber')
    if (min > 0 && max > 0 && max < min) return fail('filter', 'rssInvalidSizeRange')
    const limit = fetchLimit(form.maxPerFetch)
    if (limit === null) return fail('advanced', 'rssInvalidNumber')
    const name = form.name.trim() !== '' ? form.name.trim() : existing ? '' : (validated?.title ?? '')
    const input: RssSourceInput = {
      ...(source ?? {}),
      providerId: request.providerId ?? BUILTIN_PROVIDER_ID,
      providerConfig: request.providerConfig ?? '',
      url: request.url,
      name,
      enabled: form.enabled,
      autoDownload: form.autoDownload,
      startPaused: form.startPaused,
      intervalMinutes: form.interval,
      queueId: form.queueId,
      saveDir: form.saveDir.trim(),
      includePattern: form.include.trim(),
      excludePattern: form.exclude.trim(),
      useRegex: form.useRegex,
      smartEpisode: form.smartEpisode,
      sizeMinBytes: min,
      sizeMaxBytes: max,
      cookies: request.cookies ?? '',
      userAgent: request.userAgent ?? '',
      proxyUrl: request.proxyUrl ?? '',
      maxPerFetch: limit,
      sendReferer: form.sendReferer,
      notifyOnDownload: form.notifyOnDownload,
    }
    setError(null)
    setSaving(true)
    try {
      if (source) await rpc.daemon.rss.updateSource({ ...input, sourceId: source.sourceId })
      else await rpc.daemon.rss.createSource(input)
      onOpenChange(false)
    } catch (err) {
      setError(rpcErrorText(err))
    } finally {
      setSaving(false)
    }
  }

  const canValidate = !validating && !saving && request.url !== ''
  const canSave = !saving && !validating && (existing || validOk)

  const queueOptions = queues.map((queue) => ({ value: queue.queueId, label: queueLabel(t, queue.queueId, queue.name) }))
  if (!queueOptions.some((option) => option.value === form.queueId)) queueOptions.push({ value: form.queueId, label: form.queueId })
  const intervalValues = INTERVALS.includes(form.interval) ? INTERVALS : [...INTERVALS, form.interval]
  const intervalOptions = intervalValues.map((minutes) => ({ value: String(minutes), label: intervalLabel(t, minutes) }))

  const toggle = (title: string, description: string, checked: boolean, onChange: (value: boolean) => void) => (
    <OptionRow title={title} description={description} control={<Switch checked={checked} onCheckedChange={onChange} aria-label={title} />} />
  )

  const builtin = form.providerId === BUILTIN_PROVIDER_ID
  const currentChoice = choices.find((choice) => choice.id === form.providerId)
  const providerLabel = (choice: ProviderChoice) => (choice.kind === 'builtin' ? t('rssProviderBuiltin') : choice.kind === 'plugin' ? choice.name : choice.id)
  const providerHint =
    currentChoice?.kind === 'plugin'
      ? t('rssProviderPluginHint')
      : currentChoice?.kind === 'missing'
        ? t('rssProviderUnavailable', { id: form.providerId })
        : undefined

  const basicForm = (
    <>
      {choices.length > 1 ? (
        <FormField label={t('rssProviderLabel')} hint={providerHint}>
          <Select
            value={form.providerId}
            options={choices.map((choice) => ({ value: choice.id, label: providerLabel(choice) }))}
            onValueChange={(value) => {
              if (value !== form.providerId) {
                setValidated(null)
                patchRequest({ providerId: value })
              }
            }}
          />
        </FormField>
      ) : null}
      <FormField
        label={t(builtin ? 'rssUrlLabel' : 'rssProviderUrlLabel')}
        hint={existing ? undefined : t(validating ? 'rssWizardValidating' : 'rssEditorAuthHint')}
      >
        <InputWithAction
          input={
            <Input
              value={form.url}
              placeholder={t('rssUrlHint')}
              inputMode="url"
              autoCapitalize="off"
              autoCorrect="off"
              spellCheck={false}
              autoFocus={!existing}
              onChange={(event) => patchRequest({ url: event.target.value })}
            />
          }
          action={
            <Button variant="outline" loading={validating} disabled={!canValidate} onClick={() => void validate()}>
              {t('rssWizardValidate')}
            </Button>
          }
        />
      </FormField>
      {builtin ? null : (
        <FormField label={t('rssProviderConfigLabel')} hint={t('rssProviderConfigHint')}>
          <Input
            value={form.providerConfig}
            placeholder={t('rssProviderConfigPlaceholder')}
            autoCapitalize="off"
            autoCorrect="off"
            spellCheck={false}
            onChange={(event) => patchRequest({ providerConfig: event.target.value })}
          />
        </FormField>
      )}
      {validOk && !existing ? (
        <Card className="flex items-center gap-2 px-3 py-2">
          <Icon icon={CheckCircle2} className="text-success" />
          <div className="flex min-w-0 flex-1 flex-col">
            <span className="truncate text-sm font-medium text-foreground">{validated.title}</span>
            <span className="text-xs text-text-tertiary">{t('rssWizardFeedSummary', { n: validated.items.length })}</span>
          </div>
        </Card>
      ) : null}
      {existing || validOk ? (
        <>
          <FormField label={t('rssNameLabel')}>
            <Input value={form.name} placeholder={t('rssNameHint')} onChange={(event) => patch({ name: event.target.value })} />
          </FormField>
          <FormRow>
            <FormField label={t('rssIntervalLabel')} hint={t('rssIntervalHint')}>
              <Select value={String(form.interval)} options={intervalOptions} onValueChange={(value) => patch({ interval: Number(value) })} />
            </FormField>
            <FormField label={t('rssQueueLabel')}>
              <Select value={form.queueId} options={queueOptions} onValueChange={(value) => patch({ queueId: value })} />
            </FormField>
          </FormRow>
          <FormField label={t('rssSaveDirLabel')} hint={t('rssSaveDirHint')}>
            <InputWithAction
              input={<Input value={form.saveDir} placeholder={t('rssSaveDirHint')} onChange={(event) => patch({ saveDir: event.target.value })} />}
              action={
                <Button variant="outline" icon={FolderOpen} onClick={() => setPickingDir(true)}>
                  {t('browse')}
                </Button>
              }
            />
          </FormField>
          <div className="flex flex-col gap-1.5">
            <OptionGroup>
              {toggle(t('rssEnabledLabel'), t('rssEnabledDesc'), form.enabled, (enabled) => patch({ enabled }))}
              {toggle(t('rssAutoDownloadLabel'), t('rssAutoDownloadDesc'), form.autoDownload, (autoDownload) => patch({ autoDownload }))}
              {toggle(t('rssStartPausedLabel'), t('rssStartPausedDesc'), form.startPaused, (startPaused) => patch({ startPaused }))}
            </OptionGroup>
            <FieldHint>{t('rssWizardSeedNote')}</FieldHint>
          </div>
        </>
      ) : null}
    </>
  )

  const filterForm = (
    <>
      <FormField label={t('rssIncludeLabel')}>
        <Input value={form.include} placeholder={t('rssIncludeHint')} onChange={(event) => patch({ include: event.target.value })} />
      </FormField>
      <FormField label={t('rssExcludeLabel')}>
        <Input value={form.exclude} placeholder={t('rssExcludeHint')} onChange={(event) => patch({ exclude: event.target.value })} />
      </FormField>
      <FormRow>
        <FormField label={t('rssSizeMinLabel')}>
          <Input value={form.sizeMin} placeholder="200M" onChange={(event) => patch({ sizeMin: event.target.value })} />
        </FormField>
        <FormField label={t('rssSizeMaxLabel')}>
          <Input value={form.sizeMax} placeholder="2G" onChange={(event) => patch({ sizeMax: event.target.value })} />
        </FormField>
      </FormRow>
      <OptionGroup>
        {toggle(t('rssUseRegexLabel'), t('rssUseRegexDesc'), form.useRegex, (useRegex) => patch({ useRegex }))}
        {toggle(t('rssSmartEpisodeLabel'), t('rssSmartEpisodeDesc'), form.smartEpisode, (smartEpisode) => patch({ smartEpisode }))}
      </OptionGroup>
      <FilterPreview rule={rule} items={previewItems} />
    </>
  )

  const advancedForm = (
    <>
      <FormField label={t('rssCookiesLabel')}>
        <Input
          value={form.cookies}
          placeholder={t('rssCookiesHint')}
          autoCapitalize="off"
          autoCorrect="off"
          spellCheck={false}
          onChange={(event) => patchRequest({ cookies: event.target.value })}
        />
      </FormField>
      <FormField label={t('rssUserAgentLabel')}>
        <Input
          value={form.userAgent}
          placeholder={t('rssInheritGlobalHint')}
          autoCapitalize="off"
          autoCorrect="off"
          spellCheck={false}
          onChange={(event) => patchRequest({ userAgent: event.target.value })}
        />
      </FormField>
      <FormRow>
        <FormField label={t('rssProxyLabel')}>
          <Input
            value={form.proxyUrl}
            placeholder={t('rssInheritGlobalHint')}
            autoCapitalize="off"
            autoCorrect="off"
            spellCheck={false}
            onChange={(event) => patchRequest({ proxyUrl: event.target.value })}
          />
        </FormField>
        <FormField label={t('rssMaxPerFetchLabel')}>
          <Input value={form.maxPerFetch} inputMode="numeric" onChange={(event) => patch({ maxPerFetch: event.target.value })} />
        </FormField>
      </FormRow>
      <OptionGroup>
        {toggle(t('rssSendRefererLabel'), t('rssSendRefererDesc'), form.sendReferer, (sendReferer) => patch({ sendReferer }))}
        {toggle(t('rssNotifyLabel'), t('rssNotifyDesc'), form.notifyOnDownload, (notifyOnDownload) => patch({ notifyOnDownload }))}
      </OptionGroup>
    </>
  )

  return (
    <>
      <Dialog
        open={open}
        onOpenChange={onOpenChange}
        title={t(existing ? 'rssManageTitle' : 'rssAddSource')}
        size="lg"
        modalLocked={saving}
        footer={
          <ConfirmFooter
            okLabel={t(existing ? 'confirm' : 'rssWizardSubscribe')}
            onCancel={() => onOpenChange(false)}
            onOk={() => void save()}
            okDisabled={!canSave}
            loading={saving}
          />
        }
      >
        <Form onSubmit={() => void (canSave ? save() : validate())}>
          <SegmentedTabs<Tab>
            value={tab}
            onValueChange={setTab}
            items={[
              { value: 'basic', label: t('rssTabBasic') },
              { value: 'filter', label: t('rssTabFilter') },
              { value: 'advanced', label: t('rssTabAdvanced') },
            ]}
          />
          {tab === 'basic' ? basicForm : tab === 'filter' ? filterForm : advancedForm}
          {error ? <FieldError>{error}</FieldError> : null}
        </Form>
      </Dialog>
      <FsPickerDialog
        open={pickingDir}
        onOpenChange={setPickingDir}
        initialPath={form.saveDir.trim()}
        onSelect={(path) => {
          patch({ saveDir: path })
          setPickingDir(false)
        }}
      />
    </>
  )
}
