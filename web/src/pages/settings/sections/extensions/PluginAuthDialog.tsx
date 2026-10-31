// 插件平台登录对话框：一次登录后凭据由引擎保存，后续插件请求自动复用。
// 流程与 GPUI `plugin_auth.rs` 一致：打开即 status → begin →（二维码挑战每 2s 自动 poll；
// 其它挑战手动「检查状态」）→ success；已登录可 logout；任何方式关闭都向引擎 cancel 当前会话。

import { CircleCheck, Clock, Copy } from 'lucide-react'
import { useEffect, useId, useMemo, useRef, useState } from 'react'
import { useT } from '../../../../i18n'
import { copyText } from '../../../../lib/copy'
import { rpc } from '../../../../lib/rpc'
import type { PluginAuthRequest, PluginAuthResponse, PluginDto } from '../../../../lib/rpc'
import { Button, Dialog, DialogFooter, FieldError, FormField, Icon, Input, toast } from '../../../../ui'
import { activeAuthViewOf } from '../../../plugins/contributions'
import { PluginViewHost } from '../../../plugins/view/PluginViewHost'
import { encodeQrChallengeImage } from './challengeImage'
import { extensionErrorText } from './errors'
import { applyPluginAuthResponse, dataImageChallengeSrc, isQrcodeChallenge, safeHttpUrl, truncateChallengeText } from './logic'
import type { PluginAuthState } from './logic'

const POLL_INTERVAL_MS = 2000

type AuthAction = 'begin' | 'poll' | 'logout' | 'status'

const INITIAL: PluginAuthState = { status: '', sessionId: '', authRef: '', challenge: null, challengeType: null, message: null }

function ChallengeView({ value, type }: { value: string; type: string }) {
  const t = useT()
  // 只使用安全的 data 图片或本地编码的二维码，绝不将挑战 URL 用作图片请求。
  const dataImage = useMemo(() => dataImageChallengeSrc(value), [value])
  const qrText = !dataImage && !value.toLowerCase().startsWith('data:') && isQrcodeChallenge(type) ? value : null
  const [generated, setGenerated] = useState<{ value: string; src: string | null } | null>(null)
  useEffect(() => {
    if (qrText === null) return
    let current = true
    void encodeQrChallengeImage(qrText).then((src) => {
      if (current) setGenerated({ value: qrText, src })
    })
    return () => {
      current = false
    }
  }, [qrText])
  // 新挑战首帧便隐藏旧结果，异步编码完成后也只有当前挑战可以更新图片。
  const image = dataImage ?? (qrText !== null && generated?.value === qrText ? generated.src : null)
  const link = image || isQrcodeChallenge(type) ? null : safeHttpUrl(value)
  return (
    <div className="flex w-full min-w-0 flex-col gap-2 rounded-md bg-muted p-3">
      {type ? <div className="text-xs text-muted-foreground">{type}</div> : null}
      {image ? (
        <div className="flex w-full justify-center">
          <img
            src={image}
            alt={type}
            width={240}
            height={240}
            className="size-[240px] max-w-full rounded-sm bg-white object-contain"
          />
        </div>
      ) : (
        <>
          <div className="whitespace-pre-wrap break-all text-xs text-foreground">{truncateChallengeText(value)}</div>
          {link ? (
            <a href={link} target="_blank" rel="noopener noreferrer" className="w-fit text-xs text-accent-text hover:underline">
              {link}
            </a>
          ) : null}
        </>
      )}
      <div>
        <Button
          variant="outline"
          icon={Copy}
          onClick={() => {
            copyText(value)
            toast.success(t('apiServiceCopied'))
          }}
        >
          {t('apiServiceCopy')}
        </Button>
      </div>
    </div>
  )
}

function AuthBody({ plugin, onClose }: { plugin: PluginDto; onClose: () => void }) {
  const t = useT()
  const siteId = useId()
  const inputId = useId()
  const [auth, setAuth] = useState<PluginAuthState>(INITIAL)
  const [site, setSite] = useState('')
  const [input, setInput] = useState('')
  const [busy, setBusy] = useState(true)

  // 定时器 / 卸载清理读取最新值，避免闭包过期。
  const latest = useRef({ auth, site, input })
  latest.current = { auth, site, input }
  const busyRef = useRef(true)
  const queriedSite = useRef('')
  const alive = useRef(true)

  const send = async (action: AuthAction, siteValue: string): Promise<PluginAuthResponse> => {
    const { auth: current, input: inputValue } = latest.current
    const request: PluginAuthRequest = {
      identity: plugin.identity,
      action,
      site: siteValue,
      authRef: action === 'status' ? '' : current.authRef,
      sessionId: action === 'status' ? '' : current.sessionId,
      input: action === 'status' ? '' : inputValue,
    }
    return rpc.daemon.plugin.auth(request)
  }

  const apply = (response: PluginAuthResponse, options: { notifySuccess: boolean; wasLogout: boolean }) => {
    setAuth((previous) => applyPluginAuthResponse(previous, response, options.wasLogout))
    if (options.notifySuccess && response.status === 'success') toast.success(t('pluginAuthSuccess'))
  }

  const run = async (action: AuthAction, options: { siteValue?: string; notifySuccess?: boolean } = {}) => {
    if (busyRef.current) return
    busyRef.current = true
    setBusy(true)
    if (action !== 'poll') setAuth((previous) => ({ ...previous, message: null }))
    try {
      const response = await send(action, options.siteValue ?? latest.current.site)
      if (!alive.current) return
      if (typeof response?.status !== 'string') {
        setAuth((previous) => ({ ...previous, status: 'error', message: t('pluginAuthInvalidResponse') }))
      } else {
        apply(response, { notifySuccess: options.notifySuccess ?? false, wasLogout: action === 'logout' })
      }
    } catch (error) {
      if (alive.current) {
        setAuth((previous) => ({
          ...previous,
          status: 'error',
          message: t('pluginAuthFailed', { message: extensionErrorText(t, error) }),
        }))
      }
    } finally {
      if (alive.current) {
        busyRef.current = false
        setBusy(false)
      }
    }
  }
  const runRef = useRef(run)
  runRef.current = run

  // 打开即查询登录状态；卸载（X / Esc / 遮罩 / 取消按钮）时向引擎释放悬空会话。
  useEffect(() => {
    alive.current = true
    busyRef.current = false
    void runRef.current('status', { siteValue: '' })
    return () => {
      alive.current = false
      const { auth: current, site: siteValue } = latest.current
      if (current.sessionId === '') return
      // fire-and-forget：与在途 poll 并存无害。
      void rpc.daemon.plugin
        .auth({
          identity: plugin.identity,
          action: 'cancel',
          site: siteValue,
          authRef: current.authRef,
          sessionId: current.sessionId,
          input: '',
        })
        .catch(() => undefined)
    }
  }, [plugin.identity])

  const qrPolling = auth.status === 'pending' && auth.sessionId !== '' && isQrcodeChallenge(auth.challengeType)
  useEffect(() => {
    if (!qrPolling) return
    const timer = setInterval(() => void runRef.current('poll', { notifySuccess: true }), POLL_INTERVAL_MS)
    return () => clearInterval(timer)
  }, [qrPolling])

  // status 在不同 action 下语义不同（logout 的 success 是「注销成功」），故已登录额外要求 authRef 非空。
  const loggedIn = auth.status === 'success' && auth.authRef !== ''
  const sessionPending = auth.status === 'pending' && auth.sessionId !== ''

  const querySite = () => {
    if (busyRef.current || site === queriedSite.current) return
    queriedSite.current = site
    void run('status', { siteValue: site })
  }

  let primary = null
  if (loggedIn) {
    primary = (
      <Button variant="outline" loading={busy} onClick={() => void run('logout')}>
        {t('pluginAuthLogout')}
      </Button>
    )
  } else if (sessionPending && !isQrcodeChallenge(auth.challengeType)) {
    // 非二维码挑战没有自动轮询，提供手动「检查状态」。
    primary = (
      <Button variant="primary" loading={busy} onClick={() => void run('poll', { notifySuccess: true })}>
        {t('pluginAuthPoll')}
      </Button>
    )
  } else if (!sessionPending) {
    primary = (
      <Button variant="primary" loading={busy} onClick={() => void run('begin', { notifySuccess: true })}>
        {t('pluginAuthBegin')}
      </Button>
    )
  }

  return (
    <Dialog
      open
      onOpenChange={(open) => !open && onClose()}
      title={t('pluginAuthDialogTitle', { name: plugin.name })}
      footer={
        <DialogFooter>
          <Button variant="outline" onClick={onClose} disabled={busy}>
            {t('cancel')}
          </Button>
          {primary}
        </DialogFooter>
      }
    >
      <div className="flex w-full flex-col gap-4 py-1">
        <div className="text-xs text-muted-foreground">{t('pluginAuthDescription')}</div>
        <FormField label={t('pluginAuthSiteLabel')} htmlFor={siteId}>
          <Input
            id={siteId}
            value={site}
            disabled={busy}
            placeholder={t('pluginAuthSitePlaceholder')}
            onChange={(event) => setSite(event.target.value)}
            onBlur={querySite}
            onKeyDown={(event) => {
              if (event.key === 'Enter') querySite()
            }}
          />
        </FormField>
        <FormField label={t('pluginAuthInputLabel')} htmlFor={inputId}>
          <Input
            id={inputId}
            value={input}
            disabled={busy}
            placeholder={t('pluginAuthInputPlaceholder')}
            onChange={(event) => setInput(event.target.value)}
          />
        </FormField>
        {loggedIn ? (
          <div className="flex items-start gap-2 text-sm text-foreground">
            <Icon icon={CircleCheck} className="mt-0.5 text-success" />
            <span className="min-w-0 flex-1">{t('pluginAuthSuccess')}</span>
          </div>
        ) : sessionPending ? (
          <div className="flex items-start gap-2 text-sm text-foreground">
            <Icon icon={Clock} className="mt-0.5 text-primary" />
            <span className="min-w-0 flex-1">{t('pluginAuthPending')}</span>
          </div>
        ) : null}
        {auth.challenge !== null ? <ChallengeView value={auth.challenge} type={auth.challengeType ?? ''} /> : null}
        {auth.message ? (
          auth.status === 'error' ? (
            <FieldError>{auth.message}</FieldError>
          ) : (
            <div className="text-sm text-foreground" role="status">{auth.message}</div>
          )
        ) : null}
      </div>
    </Dialog>
  )
}

/** 插件声明了可运行的登录视图：弹窗只承载该视图（插件自己决定流程与关闭）。 */
function AuthViewBody({ plugin, view, onClose }: { plugin: PluginDto; view: string; onClose: () => void }) {
  const t = useT()
  return (
    <Dialog open onOpenChange={(open) => !open && onClose()} title={t('pluginAuthDialogTitle', { name: plugin.name })}>
      <PluginViewHost identity={plugin.identity} view={view} onClose={onClose} />
    </Dialog>
  )
}

export function PluginAuthDialog({ plugin, onClose }: { plugin: PluginDto | null; onClose: () => void }) {
  if (!plugin) return null
  const authView = activeAuthViewOf(plugin)
  if (authView) return <AuthViewBody key={plugin.identity} plugin={plugin} view={authView.id} onClose={onClose} />
  return <AuthBody key={plugin.identity} plugin={plugin} onClose={onClose} />
}
