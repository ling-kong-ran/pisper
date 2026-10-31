// [新建 ▾]（规格 5.1、计划 §5.4–5.5）：空白组件（名称 + 可选的一句话描述）或从内置模板“今日用量”“便签”开始。
// 模板只生成到工作室、不放置、不开会话；空白组件填了描述时，建好目录后由调用方交给代理生成（绑定会话）。
// 桌面是对话框，手机是提示词底部表单（规格 7.4“新建用提示词底部表单”）。
import { useId, useRef, useState } from 'react'
import { ChevronDown, Plus } from 'lucide-react'
import { COMPONENT_NAME_MAX } from './studio-model'
import { useI18n } from '@/app/use-i18n'
import { Button } from '@/components/ui/button'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { Input } from '@/components/ui/input'
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetFooter,
  SheetHeader,
  SheetTitle,
} from '@/components/ui/sheet'
import { Textarea } from '@/components/ui/textarea'
import { Label } from '@/components/ui/label'
import { useOverlayFocusReturn } from '@/hooks/useOverlayFocusReturn'
import { cn } from '@/lib/utils'
import type { ComponentRow } from '@/features/custom-ui/custom-ui-api'
import { AGENT_PROMPT_MAX } from './use-component-agent'
import type { StudioTemplate } from './use-studio-actions'

export function NewComponentMenu({
  onCreate,
  phone = false,
  className,
}: {
  // 返回新建的行（失败时为 null，提示由动作层负责）；prompt 非空时调用方建好后交给代理生成。
  onCreate: (
    template: StudioTemplate,
    name?: string,
    prompt?: string,
  ) => Promise<ComponentRow | null>
  phone?: boolean
  className?: string
}) {
  const { t } = useI18n()
  const [naming, setNaming] = useState(false)
  const [name, setName] = useState('')
  const [prompt, setPrompt] = useState('')
  const [busy, setBusy] = useState(false)
  // 对话框从菜单项打开（没有 DialogTrigger）：关闭后焦点还给 [新建 ▾]；建好后焦点交给新选中的行时不抢。
  const triggerRef = useRef<HTMLButtonElement>(null)
  const returnFocus = useOverlayFocusReturn(naming, triggerRef)
  const nameId = useId()
  const promptId = useId()
  const create = async (template: StudioTemplate, value?: string, description?: string) => {
    setBusy(true)
    try {
      const row = await onCreate(template, value, description)
      if (row) {
        setNaming(false)
        setName('')
        setPrompt('')
      }
    } finally {
      setBusy(false)
    }
  }
  const trimmed = name.trim()
  const described = prompt.trim()
  const submit = () => {
    if (trimmed) void create('blank', trimmed, described || undefined)
  }
  // 可见标签（占位文字一输入就消失，不能当标签用）。
  const fields = (
    <>
      <Label htmlFor={nameId} className="-mb-1.5 text-xs text-content-secondary">
        {t('custom-ui:studio.componentName')}
      </Label>
      <Input
        id={nameId}
        autoFocus
        value={name}
        maxLength={COMPONENT_NAME_MAX}
        placeholder={t('custom-ui:studio.componentNamePlaceholder')}
        data-component-new-name
        className={phone ? 'min-h-11' : undefined}
        onChange={(event) => setName(event.target.value)}
      />
      <Label htmlFor={promptId} className="-mb-1.5 text-xs text-content-secondary">
        {t('custom-ui:agent.newPromptLabel')}
      </Label>
      <Textarea
        id={promptId}
        rows={3}
        value={prompt}
        maxLength={AGENT_PROMPT_MAX}
        placeholder={t('custom-ui:agent.newPromptPlaceholder')}
        data-component-new-prompt
        className="max-h-48 resize-none"
        onChange={(event) => setPrompt(event.target.value)}
        onKeyDown={(event) => {
          if (event.key !== 'Enter' || !(event.metaKey || event.ctrlKey)) return
          if (event.nativeEvent.isComposing) return
          event.preventDefault()
          submit()
        }}
      />
    </>
  )
  const submitLabel = described
    ? t('custom-ui:agent.createAndGenerate')
    : t('custom-ui:studio.create')
  return (
    <>
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button
            ref={triggerRef}
            size="sm"
            variant="secondary"
            disabled={busy}
            data-component-new
            className={cn(className)}
          >
            <Plus />
            {t('custom-ui:studio.new')}
            <ChevronDown className="text-content-muted" />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end" className="w-56">
          <DropdownMenuItem data-component-new-blank onSelect={() => setNaming(true)}>
            {t('custom-ui:studio.newBlank')}
          </DropdownMenuItem>
          <DropdownMenuSeparator />
          <DropdownMenuLabel className="text-xs font-normal text-content-muted">
            {t('custom-ui:studio.fromTemplate')}
          </DropdownMenuLabel>
          <DropdownMenuItem
            data-component-new-template="usage-today"
            onSelect={() => void create('usage-today')}
          >
            {t('custom-ui:studio.templateUsageToday')}
          </DropdownMenuItem>
          <DropdownMenuItem
            data-component-new-template="notes"
            onSelect={() => void create('notes')}
          >
            {t('custom-ui:studio.templateNotes')}
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
      {phone ? (
        <Sheet open={naming} onOpenChange={(open) => !busy && setNaming(open)}>
          <SheetContent
            side="bottom"
            onCloseAutoFocus={returnFocus}
            data-component-new-sheet
            className="gap-0 rounded-t-[var(--r-lg)] p-0"
          >
            <form
              className="flex flex-col gap-3 px-4 pt-4 pb-[max(16px,env(safe-area-inset-bottom))]"
              onSubmit={(event) => {
                event.preventDefault()
                submit()
              }}
            >
              <SheetHeader className="p-0">
                <SheetTitle>{t('custom-ui:studio.newBlankTitle')}</SheetTitle>
                <SheetDescription>{t('custom-ui:studio.newBlankHint')}</SheetDescription>
              </SheetHeader>
              {fields}
              <SheetFooter className="flex-row p-0">
                <Button
                  type="button"
                  variant="outline"
                  className="min-h-11 flex-1"
                  disabled={busy}
                  onClick={() => setNaming(false)}
                >
                  {t('custom-ui:studio.cancel')}
                </Button>
                <Button type="submit" className="min-h-11 flex-1" disabled={busy || !trimmed}>
                  {submitLabel}
                </Button>
              </SheetFooter>
            </form>
          </SheetContent>
        </Sheet>
      ) : (
        <Dialog open={naming} onOpenChange={(open) => !busy && setNaming(open)}>
          <DialogContent onCloseAutoFocus={returnFocus}>
            <form
              className="contents"
              onSubmit={(event) => {
                event.preventDefault()
                submit()
              }}
            >
              <DialogHeader>
                <DialogTitle>{t('custom-ui:studio.newBlankTitle')}</DialogTitle>
                <DialogDescription>{t('custom-ui:studio.newBlankHint')}</DialogDescription>
              </DialogHeader>
              {fields}
              <DialogFooter>
                <Button
                  type="button"
                  variant="outline"
                  disabled={busy}
                  onClick={() => setNaming(false)}
                >
                  {t('custom-ui:studio.cancel')}
                </Button>
                <Button type="submit" disabled={busy || !trimmed}>
                  {submitLabel}
                </Button>
              </DialogFooter>
            </form>
          </DialogContent>
        </Dialog>
      )}
    </>
  )
}
