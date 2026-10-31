// 分享链接二维码：本地编码（`qrcode` 已在依赖里，复用扩展 / 套餐页同一入口），不请求任何外部服务。
// 编码失败（超长 / 无 canvas）时不渲染，链接与取件码照常可复制。

import { useEffect, useState } from 'react'
import { encodeQrChallengeImage } from '../settings/sections/extensions/challengeImage'

export function QrImage({ value, label, className }: { value: string; label: string; className?: string }) {
  const [source, setSource] = useState<{ value: string; src: string | null } | null>(null)

  useEffect(() => {
    let cancelled = false
    void encodeQrChallengeImage(value).then((src) => {
      if (!cancelled) setSource({ value, src })
    })
    return () => {
      cancelled = true
    }
  }, [value])

  if (!source || source.value !== value || source.src === null) return null
  // 二维码恒为黑白高对比，不随主题反色：深色主题下深底扫码会失败。
  return <img src={source.src} alt={label} width={144} height={144} className={className ?? 'size-36 shrink-0 rounded-md bg-white p-1'} />
}
