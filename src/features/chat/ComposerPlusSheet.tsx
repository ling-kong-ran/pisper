// 手机上的输入框“+”：底部表单（规格 7.2，芯片都点开各自的底部表单），不再是浮在消息上的下拉菜单。
// 手机的麦克风键在工具行，这里没有语音；审批芯片默认收起，从这里进同一个表单。
import type { ReactElement } from 'react'
import { Braces, ChevronRight, CornerDownRight, ImagePlus, Paperclip, Shield } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { PopoverSheet } from '@/components/app/PopoverSheet'
import { SheetRow, SheetTile, SheetTileGrid } from '@/components/app/SheetActions'

export function ComposerPlusSheet({
  trigger,
  open,
  onOpenChange,
  onAttachment,
  onResource,
  onVisual,
  steer,
  mode,
}: {
  trigger: ReactElement
  open: boolean
  onOpenChange: (open: boolean) => void
  onAttachment: () => void
  onResource: () => void
  // 没有生图能力时不传。
  onVisual?: () => void
  // 运行中才传：触屏没有 ⌥↵，插话发送放在这里。
  steer?: { disabled: boolean; onSelect: () => void }
  // Bot 会话没有审批与执行方式，不传。
  mode?: { valueLabel: string; onOpen: () => void }
}) {
  const { t } = useI18n()
  // 关上这张表单之后再打开下一层：两个 Radix 弹层在同一帧开合时，焦点会回到正在关闭的那个。
  const closeThen = (next: () => void) => {
    onOpenChange(false)
    window.setTimeout(next, 0)
  }
  return (
    <PopoverSheet
      sheet
      title={t('board:composer.plusSheetTitle')}
      open={open}
      onOpenChange={onOpenChange}
      trigger={trigger}
      sheetClassName="px-2"
    >
      <SheetTileGrid count={onVisual ? 3 : 2}>
        <SheetTile
          icon={Paperclip}
          label={t('board:composer.attachmentShort')}
          data-composer-plus-item="attachment"
          onClick={() => {
            // 文件选择要在点击的调用栈里同步打开，部分 WebView 否则不认用户手势。
            onOpenChange(false)
            onAttachment()
          }}
        />
        <SheetTile
          icon={Braces}
          label={t('chat:resourcePicker.open')}
          data-composer-plus-item="resource"
          onClick={() => closeThen(onResource)}
        />
        {onVisual && (
          <SheetTile
            icon={ImagePlus}
            label={t('chat:focusSession.generateImage')}
            data-composer-plus-item="visual"
            onClick={() => closeThen(onVisual)}
          />
        )}
      </SheetTileGrid>
      {steer && (
        <SheetRow
          icon={CornerDownRight}
          label={t('board:composer.steerNow')}
          hint={t('board:composer.steerHint')}
          disabled={steer.disabled}
          data-composer-steer
          onClick={() => closeThen(steer.onSelect)}
        />
      )}
      {mode && (
        <SheetRow
          icon={Shield}
          label={t('board:composer.modeChip')}
          data-composer-plus-item="mode"
          trailing={
            <span className="flex flex-none items-center gap-1 text-sm text-content-muted">
              {mode.valueLabel}
              <ChevronRight className="size-4" aria-hidden="true" />
            </span>
          }
          onClick={() => closeThen(mode.onOpen)}
        />
      )}
    </PopoverSheet>
  )
}
