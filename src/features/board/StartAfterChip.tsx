// 派发对话框与主页草稿的“排在…之后”芯片（规格 8.4，A9，能力 sessionQueue）：默认“立即开始”；
// 选一个运行中或排队中的会话，新任务就等它这一轮结束（完成、失败、停止都算）再由 Runtime 启动。
// 没有 sessionQueue 能力、或眼下没有可排的会话（也没选过）时整枚芯片不出现（规格 8.6 降级矩阵）。
import { ChevronDown, ListOrdered } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { Chip } from '@/components/ui/chip'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuLabel,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { cn } from '@/lib/utils'
import { useBoardItems } from './useBoardItems'

export function StartAfterChip({
  value,
  onChange,
  size,
  compact = false,
  className,
}: {
  // 空串表示立即开始。
  value: string
  onChange: (sessionId: string) => void
  size: 'default' | 'sm'
  compact?: boolean
  className?: string
}) {
  const { t } = useI18n()
  const { items, capabilities } = useBoardItems('today')
  if (!capabilities.sessionQueue) return null
  // 只有还会“结束一次”的会话才值得排在它后面：运行中的，和本身在排队的（排队链）。
  const candidates = items.filter((item) => item.lane === 'running' || item.lane === 'queued')
  const selected = items.find((item) => item.id === value)
  if (!candidates.length && !value) return null
  const title = selected?.title ?? t('board:launch.startAfterGone')
  // 芯片上写短的“等「X」”（工具行放不下整句），悬停与读屏用完整的“排在 X 之后”。
  const label = value ? t('board:launch.startAfterChip', { title }) : t('board:launch.startNowChip')
  const shortLabel = value ? t('board:launch.startAfterShort', { title }) : label
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Chip
          size={size}
          className={cn(
            'min-w-0 max-w-[24ch] shrink',
            // 选了会话时至少留出能认出标题的宽度。
            value ? 'min-w-[14ch]' : 'border-transparent bg-transparent',
            className,
          )}
          title={label}
          aria-label={`${t('board:launch.startAfterLabel')} · ${label}`}
          data-launch-start-after={value || 'now'}
        >
          <ListOrdered aria-hidden="true" />
          {/* 手机底部表单只留图标；选了会话时仍写出它的标题，免得看不出这条任务会等。 */}
          {(!compact || value) && <span className="min-w-0 truncate">{shortLabel}</span>}
          <ChevronDown aria-hidden="true" />
        </Chip>
      </DropdownMenuTrigger>
      <DropdownMenuContent
        align="start"
        className="max-h-[min(24rem,var(--radix-dropdown-menu-content-available-height))] w-72 overflow-y-auto"
      >
        <DropdownMenuLabel>{t('board:launch.startAfterLabel')}</DropdownMenuLabel>
        <DropdownMenuRadioGroup value={value} onValueChange={onChange}>
          <DropdownMenuRadioItem value="">{t('board:launch.startNowChip')}</DropdownMenuRadioItem>
          {candidates.map((item) => (
            <DropdownMenuRadioItem key={item.id} value={item.id} data-start-after-option={item.id}>
              <span className="flex min-w-0 flex-col">
                <span className="truncate">{item.title}</span>
                <span className="text-xs text-content-muted">
                  {item.lane === 'running'
                    ? t('board:launch.startAfterRunning')
                    : t('board:launch.startAfterQueued')}
                </span>
              </span>
            </DropdownMenuRadioItem>
          ))}
        </DropdownMenuRadioGroup>
        <p className="px-2 py-1.5 text-xs leading-relaxed text-content-muted">
          {t('board:launch.startAfterHint')}
        </p>
      </DropdownMenuContent>
    </DropdownMenu>
  )
}
