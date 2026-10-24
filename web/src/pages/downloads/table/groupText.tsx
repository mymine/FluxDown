// 任务组「文件夹行」的共用小件组件（桌面表格与窄屏卡片共用）：计数行、组图标。

import { Layers } from 'lucide-react'
import { Fragment } from 'react'
import { cn } from '../../../lib/cn'
import { Icon } from '../../../ui'
import type { CountSegment, CountTone } from './groupFacts'

const TONE_CLASS: Record<CountTone, string> = {
  plain: '',
  accent: 'text-accent-text',
  destructive: 'text-destructive',
}

/** 计数行：` · ` 连接，强调色 / destructive 色按段着色，其余继承三级色。 */
export function CountsLine({ segments, className }: { segments: readonly CountSegment[]; className?: string }) {
  return (
    <span className={cn('tabular', className)}>
      {segments.map((segment, index) => (
        <Fragment key={index}>
          {index > 0 ? ' · ' : null}
          <span className={TONE_CLASS[segment.tone]}>{segment.text}</span>
        </Fragment>
      ))}
    </span>
  )
}

/** 组图标：强调色淡底圆角方块，右下角计数徽标 = 通过成员数。 */
export function GroupGlyph({ count }: { count: number }) {
  return (
    <div className="relative flex size-6 items-center justify-center rounded-md bg-accent-text/12">
      <Icon icon={Layers} size="md" className="text-accent-text" />
      <span className="tabular absolute -bottom-1 -right-1.5 min-w-4 rounded-full bg-primary px-1 text-center text-[10px] leading-4 text-primary-foreground">
        {count}
      </span>
    </div>
  )
}
