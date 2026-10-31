// 站点代理规则新增 / 编辑对话框（crates/settings/src/sections/proxy_rule_dialog.rs）。
// 检测一律交给 engine（`daemon.config.proxyRoute`），这里不做本地匹配：
// - 失焦 / 「试一试」：只检测草稿本身（下标 0）；
// - 保存：检测「保存后的完整列表」，只看草稿所在下标的 issue，有 issue 不写入也不关闭。

import { CircleCheck } from 'lucide-react'
import { useRef, useState } from 'react'
import { useT } from '../../../../i18n'
import { cn } from '../../../../lib/cn'
import { rpc } from '../../../../lib/rpc'
import { rpcErrorText } from '../../../../lib/rpcErrorText'
import {
  RULE_ACTIONS,
  encodeProxyRules,
  issueText,
  manualEndpoint,
  removeRule,
  upsertRule,
  upserted,
} from '../../../../lib/proxyRules'
import type { ProxyRule, RuleAction, RuleMatch } from '../../../../lib/proxyRules'
import { Button, ConfirmFooter, Dialog, FieldHint, FormField, Icon, Input, InputWithAction, SegmentedTabs, confirmDialog } from '../../../../ui'
import { useDaemonValue } from '../../kit'

const ACTION_LABEL: Record<RuleAction, string> = {
  direct: 'proxyRuleActionDirect',
  system: 'proxyRuleActionSystem',
  manual: 'proxyRuleActionManual',
  custom: 'proxyRuleActionCustom',
}

export function ProxyRuleDialog({
  existing,
  list,
  onClose,
}: {
  existing: ProxyRule | null
  /** 打开时的当前规则列表（保存前检测用它拼出完整列表）。 */
  list: readonly ProxyRule[]
  onClose: () => void
}) {
  const t = useT()
  // 新建的 id 只生成一次：之后每次检测与保存都用它。
  const newId = useRef(`pr_${Date.now()}`)
  const seq = useRef(0)

  const [name, setName] = useState(existing?.name ?? '')
  const [match, setMatch] = useState<RuleMatch>(existing?.match ?? 'domain')
  const [pattern, setPattern] = useState(existing?.pattern ?? '')
  const [action, setAction] = useState<RuleAction>(existing?.action ?? 'direct')
  const [proxy, setProxy] = useState(existing?.proxy ?? '')
  const [sample, setSample] = useState('')
  const [nameError, setNameError] = useState('')
  const [patternError, setPatternError] = useState('')
  const [proxyError, setProxyError] = useState('')
  const [formError, setFormError] = useState('')
  const [tryResult, setTryResult] = useState<boolean | null>(null)
  const [checking, setChecking] = useState(false)

  const proxyType = useDaemonValue('proxy_type')
  const proxyHost = useDaemonValue('proxy_host')
  const proxyPort = useDaemonValue('proxy_port')
  const manual = manualEndpoint(proxyType, proxyHost, proxyPort)

  const buildDraft = (): ProxyRule => ({
    id: existing?.id ?? newId.current,
    enabled: existing?.enabled ?? true,
    name: name.trim(),
    match,
    pattern: pattern.trim(),
    action,
    proxy: action === 'custom' ? proxy.trim() : '',
  })

  const check = async (thenSave: boolean) => {
    if (checking) return
    const draft = buildDraft()
    const next = thenSave ? upserted(list, draft) : [{ ...draft, enabled: true }]
    const index = thenSave ? next.findIndex((rule) => rule.id === draft.id) : 0
    const url = thenSave ? '' : sample.trim()
    const mine = ++seq.current
    setChecking(true)
    try {
      const response = await rpc.daemon.config.proxyRoute({ url, rules: encodeProxyRules(next) })
      if (mine !== seq.current) return
      const own = response.issues.filter((issue) => issue.index === index)
      const fieldText = (field: string) => {
        const issue = own.find((entry) => entry.field === field)
        return issue ? issueText(t, issue) : ''
      }
      setNameError(fieldText('name'))
      setPatternError(fieldText('pattern'))
      setProxyError(fieldText('proxy'))
      const other = own.find((issue) => !['name', 'pattern', 'proxy'].includes(issue.field))
      setFormError(other ? issueText(t, other) : '')
      if (!thenSave && url !== '') setTryResult(own.length === 0 ? response.matched != null : null)
      if (thenSave && own.length === 0) {
        try {
          await upsertRule(draft)
        } catch (error) {
          setFormError(rpcErrorText(error, t))
          return
        }
        onClose()
      }
    } catch (error) {
      if (mine === seq.current) setFormError(rpcErrorText(error, t))
    } finally {
      if (mine === seq.current) setChecking(false)
    }
  }

  const remove = async () => {
    if (!existing) return
    const ok = await confirmDialog({
      title: t('proxyRuleDeleteConfirm', { name: existing.name || existing.pattern }),
      description: t('proxyRuleDeleteDesc'),
      intent: 'destructive',
      okLabel: t('delete'),
    })
    if (!ok) return
    try {
      await removeRule(existing.id)
      onClose()
    } catch (error) {
      setFormError(rpcErrorText(error, t))
    }
  }

  const regex = match === 'regex'
  const actionHint =
    action === 'direct' ? (
      <FieldHint>{t('proxyRuleActionDirectHint')}</FieldHint>
    ) : action === 'system' ? (
      <FieldHint>{t('proxyRuleActionSystemHint')}</FieldHint>
    ) : action === 'manual' ? (
      manual === '' ? (
        <div className="text-xs text-warning">{t('proxyRuleActionManualMissing')}</div>
      ) : (
        <FieldHint>{t('proxyRuleActionManualHint', { endpoint: manual })}</FieldHint>
      )
    ) : null

  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open && !checking) onClose()
      }}
      size="lg"
      title={t(existing ? 'proxyRuleEdit' : 'proxyRuleNew')}
      footer={
        <ConfirmFooter
          okLabel={t('proxyRuleSave')}
          loading={checking}
          okDisabled={checking}
          onCancel={onClose}
          onOk={() => void check(true)}
          extra={
            existing ? (
              <Button variant="ghost" className="text-destructive" disabled={checking} onClick={() => void remove()}>
                {t('delete')}
              </Button>
            ) : undefined
          }
        />
      }
    >
      <div className="flex flex-col gap-4">
        <FormField label={t('proxyRuleName')} htmlFor="proxy-rule-name" error={nameError || undefined}>
          <Input
            id="proxy-rule-name"
            value={name}
            placeholder={t('proxyRuleNamePlaceholder')}
            invalid={nameError !== ''}
            onChange={(event) => {
              setName(event.target.value)
              setNameError('')
            }}
          />
        </FormField>

        <FormField label={t('proxyRuleMatch')}>
          <SegmentedTabs
            aria-label={t('proxyRuleMatch')}
            value={match}
            items={[
              { value: 'domain', label: t('proxyRuleMatchDomain') },
              { value: 'regex', label: t('proxyRuleMatchRegex') },
            ]}
            onValueChange={(value) => {
              setMatch(value)
              setPatternError('')
            }}
          />
        </FormField>

        <FormField
          label={t(regex ? 'proxyRuleMatchRegex' : 'proxyRuleMatchDomain')}
          htmlFor="proxy-rule-pattern"
          error={patternError || undefined}
          hint={t(regex ? 'proxyRulePatternRegexHint' : 'proxyRulePatternDomainHint')}
        >
          <Input
            id="proxy-rule-pattern"
            className="font-mono"
            value={pattern}
            autoCapitalize="off"
            autoCorrect="off"
            spellCheck={false}
            placeholder={t(regex ? 'proxyRulePatternRegexPlaceholder' : 'proxyRulePatternDomainPlaceholder')}
            invalid={patternError !== ''}
            onChange={(event) => {
              setPattern(event.target.value)
              setPatternError('')
            }}
            onBlur={() => void check(false)}
          />
        </FormField>

        <FormField label={t('proxyRuleAction')}>
          <div className="flex flex-wrap gap-1.5" role="group" aria-label={t('proxyRuleAction')}>
            {RULE_ACTIONS.map((value) => (
              <button
                key={value}
                type="button"
                aria-pressed={value === action}
                onClick={() => {
                  setAction(value)
                  setProxyError('')
                }}
                className={cn(
                  'inline-flex h-control items-center rounded-md border px-3 text-sm transition-colors coarse:min-h-touch',
                  value === action ? 'border-primary bg-accent font-medium text-accent-text' : 'border-border text-foreground hover:bg-row-hover',
                )}
              >
                {t(ACTION_LABEL[value])}
              </button>
            ))}
          </div>
          {actionHint}
        </FormField>

        {action === 'custom' ? (
          <FormField
            label={t('proxyRuleProxyUrl')}
            htmlFor="proxy-rule-proxy"
            error={proxyError || undefined}
            hint={t('proxyRuleProxyUrlHint')}
          >
            <Input
              id="proxy-rule-proxy"
              className="font-mono"
              value={proxy}
              autoCapitalize="off"
              autoCorrect="off"
              spellCheck={false}
              placeholder={t('proxyRuleProxyUrlPlaceholder')}
              invalid={proxyError !== ''}
              onChange={(event) => {
                setProxy(event.target.value)
                setProxyError('')
              }}
              onBlur={() => void check(false)}
            />
          </FormField>
        ) : null}

        <FormField label={t('proxyRuleTry')} htmlFor="proxy-rule-sample">
          <InputWithAction
            input={
              <Input
                id="proxy-rule-sample"
                value={sample}
                inputMode="url"
                autoCapitalize="off"
                autoCorrect="off"
                spellCheck={false}
                placeholder={t('proxyRouteUrlPlaceholder')}
                onChange={(event) => {
                  setSample(event.target.value)
                  setTryResult(null)
                }}
                onKeyDown={(event) => {
                  if (event.key === 'Enter' && sample.trim() !== '') void check(false)
                }}
              />
            }
            action={
              <Button disabled={checking || sample.trim() === ''} onClick={() => void check(false)}>
                {t('proxyRouteCheck')}
              </Button>
            }
          />
          {tryResult === true ? (
            <div className="flex items-center gap-1.5 text-xs text-success">
              <Icon icon={CircleCheck} size="md" />
              {t('proxyRuleTryHit')}
            </div>
          ) : tryResult === false ? (
            <div className="text-xs text-text-tertiary">{t('proxyRuleTryMiss')}</div>
          ) : null}
        </FormField>

        {formError ? <div className="text-xs text-destructive">{formError}</div> : null}
      </div>
    </Dialog>
  )
}
