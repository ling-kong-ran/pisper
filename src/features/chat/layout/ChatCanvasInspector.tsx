import { useId } from 'react'
import { ArrowDown, ArrowUp, Copy, Trash2 } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { AppSelect } from '@/components/AppSelect'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Textarea } from '@/components/ui/textarea'
import { CANVAS_TEXT_MAX_LENGTH, type ChatCanvasNode } from './chat-canvas'
import { CANVAS_CSS_MAX_LENGTH, parseCanvasCss } from './chat-canvas-style'
import {
  canCopyCanvasNode,
  canRemoveCanvasNode,
  canvasKindLabels,
  canvasParent,
  replaceCanvasCssDeclaration,
} from './chat-canvas-editor-model'

export function ChatCanvasInspector({
  root,
  node,
  css,
  cssError,
  parents,
  onCss,
  onText,
  onMove,
  onCopy,
  onDelete,
  onMoveTo,
}: {
  root: ChatCanvasNode
  node: ChatCanvasNode
  css: string
  cssError: string
  parents: ChatCanvasNode[]
  onCss: (css: string) => void
  onText: (text: string) => void
  onMove: (delta: number) => void
  onCopy: () => void
  onDelete: () => void
  onMoveTo: (parentId: string) => void
}) {
  const { t } = useI18n()
  const id = useId()
  const labels = canvasKindLabels(t)
  const parent = canvasParent(root, node.id)
  const index = parent?.children?.findIndex((entry) => entry.id === node.id) ?? 0
  const deletable = canRemoveCanvasNode(root, node)
  let style: ReturnType<typeof parseCanvasCss> = {}
  try {
    style = parseCanvasCss(css)
  } catch {
    /* 无效声明保留在编辑框中，不覆盖已应用的画布样式。 */
  }
  const setProperty = (property: string, value: string) => {
    if (cssError) return
    try {
      onCss(replaceCanvasCssDeclaration(css, property, value))
    } catch {
      // 长度限制仍由同一验证入口处理，保留原有声明供用户修正。
      onCss(`${css}\n${property}: ${value};`)
    }
  }
  return (
    <div className="min-w-0 space-y-4">
      <div>
        <h3 className="text-sm font-medium">{labels[node.kind]}</h3>
        <p className="mt-1 break-all font-mono text-[10px] text-muted-foreground">{node.id}</p>
      </div>
      <div className="flex flex-wrap gap-1.5">
        <Button
          variant="outline"
          size="icon"
          disabled={!parent || index === 0}
          onClick={() => onMove(-1)}
          aria-label={t('chat-layout:canvas.moveUp')}
          title={t('chat-layout:canvas.moveUp')}
        >
          <ArrowUp />
        </Button>
        <Button
          variant="outline"
          size="icon"
          disabled={!parent || index >= (parent.children?.length ?? 0) - 1}
          onClick={() => onMove(1)}
          aria-label={t('chat-layout:canvas.moveDown')}
          title={t('chat-layout:canvas.moveDown')}
        >
          <ArrowDown />
        </Button>
        <Button
          variant="outline"
          size="icon"
          disabled={!parent || !canCopyCanvasNode(node)}
          onClick={onCopy}
          aria-label={t('chat-layout:canvas.copy')}
          title={t('chat-layout:canvas.copy')}
        >
          <Copy />
        </Button>
        <Button
          variant="outline"
          size="icon"
          disabled={!deletable}
          onClick={onDelete}
          aria-label={t('chat-layout:canvas.delete')}
          title={t('chat-layout:canvas.delete')}
        >
          <Trash2 />
        </Button>
      </div>
      {!deletable && (
        <p className="text-xs leading-5 text-muted-foreground">
          {t('chat-layout:canvas.requiredHint')}
        </p>
      )}
      {parent && (
        <div className="space-y-2">
          <Label id={`${id}-parent`}>{t('chat-layout:canvas.moveInto')}</Label>
          <AppSelect
            aria-labelledby={`${id}-parent`}
            value={parent.id}
            onChange={(event) => onMoveTo(event.target.value)}
          >
            {parents.map((entry) => (
              <option key={entry.id} value={entry.id}>
                {labels[entry.kind]} · {entry.id}
              </option>
            ))}
          </AppSelect>
        </div>
      )}
      {node.kind === 'text' && (
        <div className="min-w-0 space-y-2">
          <Label htmlFor={`${id}-text`}>{t('chat-layout:canvas.textContent')}</Label>
          <Textarea
            id={`${id}-text`}
            value={node.text ?? ''}
            maxLength={CANVAS_TEXT_MAX_LENGTH}
            onChange={(event) => onText(event.currentTarget.value)}
            className="min-w-0 field-sizing-fixed"
          />
        </div>
      )}
      <div className="min-w-0 space-y-2">
        <Label htmlFor={`${id}-css`}>{t('chat-layout:canvas.css')}</Label>
        <Textarea
          id={`${id}-css`}
          value={css}
          maxLength={CANVAS_CSS_MAX_LENGTH}
          spellCheck={false}
          aria-invalid={Boolean(cssError)}
          aria-describedby={cssError ? `${id}-css-error` : `${id}-css-hint`}
          onChange={(event) => onCss(event.currentTarget.value)}
          className="h-56 max-h-[45dvh] min-w-0 w-full resize-y field-sizing-fixed font-mono text-xs [overflow-wrap:anywhere]"
          placeholder={'padding: 16px;\nborder-radius: 12px;\nbackground: var(--muted);'}
        />
        {cssError && (
          <p id={`${id}-css-error`} role="alert" className="text-xs leading-5">
            {cssError}
          </p>
        )}
        <p id={`${id}-css-hint`} className="text-xs leading-5 text-muted-foreground">
          {t('chat-layout:canvas.cssHint')}
        </p>
      </div>
      <details className="space-y-3 border-t border-border pt-3">
        <summary className="cursor-pointer text-xs font-medium">
          {t('chat-layout:canvas.quickStyles')}
        </summary>
        <fieldset disabled={Boolean(cssError)} className="min-w-0 space-y-3 disabled:opacity-50">
          <div className="grid grid-cols-2 gap-3">
            <div className="space-y-2">
              <Label htmlFor={`${id}-bg`} className="text-xs">
                {t('chat-layout:canvas.background')}
              </Label>
              <Input
                id={`${id}-bg`}
                type="color"
                value={
                  typeof style.backgroundColor === 'string' &&
                  /^#[a-f\d]{6}$/i.test(style.backgroundColor)
                    ? style.backgroundColor
                    : '#ffffff'
                }
                onChange={(event) => setProperty('background-color', event.currentTarget.value)}
                className="w-full p-1"
              />
            </div>
            <div className="space-y-2">
              <Label htmlFor={`${id}-color`} className="text-xs">
                {t('chat-layout:canvas.color')}
              </Label>
              <Input
                id={`${id}-color`}
                type="color"
                value={
                  typeof style.color === 'string' && /^#[a-f\d]{6}$/i.test(style.color)
                    ? style.color
                    : '#18181b'
                }
                onChange={(event) => setProperty('color', event.currentTarget.value)}
                className="w-full p-1"
              />
            </div>
          </div>
          <div className="flex flex-wrap gap-2">
            <Button variant="outline" size="sm" onClick={() => setProperty('padding', '16px')}>
              {t('chat-layout:canvas.padding')}
            </Button>
            <Button
              variant="outline"
              size="sm"
              onClick={() => setProperty('border-radius', '12px')}
            >
              {t('chat-layout:canvas.radius')}
            </Button>
            <Button variant="outline" size="sm" onClick={() => setProperty('gap', '12px')}>
              {t('chat-layout:canvas.gap')}
            </Button>
          </div>
        </fieldset>
      </details>
    </div>
  )
}
