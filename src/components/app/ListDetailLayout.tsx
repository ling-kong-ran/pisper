import { useLayoutEffect, useRef, useState, type ReactNode } from 'react'
import { useBackLayer } from '@/hooks/use-back-layer'
import { ArrowLeft } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { cn } from '@/lib/utils'

export function ListDetailLayout({
  list,
  detail,
  inspector,
  selected,
  onBack,
  backLabel,
}: {
  list: ReactNode
  detail: ReactNode
  inspector?: ReactNode
  selected: boolean
  onBack: () => void
  backLabel: string
}) {
  const listRoot = useRef<HTMLDivElement>(null)
  const detailRoot = useRef<HTMLDivElement>(null)
  const backButton = useRef<HTMLButtonElement>(null)
  const listTrigger = useRef<HTMLElement | null>(null)
  const previous = useRef({ selected, narrow: false })
  const root = useRef<HTMLDivElement>(null)
  const [narrow, setNarrow] = useState(false)
  useLayoutEffect(() => {
    const element = root.current
    if (!element) return
    const measure = () => setNarrow(element.clientWidth < 760)
    measure()
    const observer = new ResizeObserver(measure)
    observer.observe(element)
    return () => observer.disconnect()
  }, [])
  useLayoutEffect(() => {
    const before = previous.current
    previous.current = { selected, narrow }
    const active = document.activeElement
    const restoreList = () => {
      const trigger = listTrigger.current
      if (
        trigger?.isConnected &&
        trigger.getClientRects().length &&
        !trigger.hasAttribute('disabled')
      )
        trigger.focus()
      else
        (
          Array.from(
            listRoot.current?.querySelectorAll<HTMLElement>(
              'button:not([disabled]),a[href],input:not([disabled]),[tabindex="0"]',
            ) || [],
          ).find((el) => el.getClientRects().length) || listRoot.current
        )?.focus()
    }
    if (
      narrow &&
      selected &&
      (!before.selected || (!before.narrow && listRoot.current?.contains(active)))
    )
      backButton.current?.focus()
    else if (
      narrow &&
      !selected &&
      (before.selected || (!before.narrow && detailRoot.current?.contains(active)))
    )
      restoreList()
    else if (!narrow && before.narrow && active === backButton.current) detailRoot.current?.focus()
  }, [selected, narrow])
  useBackLayer(narrow && selected, 100, onBack)
  return (
    <div
      ref={root}
      data-page-template="list-detail-layout"
      data-selected={selected}
      className="@container/list-detail flex-1 h-full min-h-0 min-w-0 overflow-hidden"
    >
      {/* 列表卡片含标题、说明与操作，低于 280px 会把标题折成多行。 */}
      <div className="grid h-full min-h-0 grid-cols-1 @min-[760px]/list-detail:grid-cols-[clamp(280px,32%,360px)_minmax(0,1fr)]">
        <div
          ref={listRoot}
          tabIndex={-1}
          onFocusCapture={(event) => {
            if (event.target instanceof HTMLElement) listTrigger.current = event.target
          }}
          data-slot="page-list"
          className={cn(
            'min-h-0 min-w-0 overflow-y-auto p-3 @min-[760px]/list-detail:border-r @min-[760px]/list-detail:border-stroke-soft',
            selected && 'hidden @min-[760px]/list-detail:block',
          )}
        >
          {list}
        </div>
        <div
          ref={detailRoot}
          tabIndex={-1}
          data-slot="page-detail"
          className={cn(
            'min-h-0 min-w-0 overflow-y-auto p-4 @min-[760px]/list-detail:px-6 @min-[760px]/list-detail:py-5',
            !selected && 'hidden @min-[760px]/list-detail:block',
          )}
        >
          <Button
            ref={backButton}
            variant="ghost"
            className="mb-3 min-h-11 @min-[760px]/list-detail:hidden"
            onClick={onBack}
          >
            <ArrowLeft />
            {backLabel}
          </Button>
          <div
            className={cn(
              'grid min-w-0 gap-4',
              inspector && '@min-[1120px]/list-detail:grid-cols-[minmax(0,1fr)_280px]',
            )}
          >
            <div className="min-w-0">{detail}</div>
            {inspector && (
              <aside
                data-slot="page-inspector"
                className="min-w-0 border-t border-stroke-soft pt-4 @min-[1120px]/list-detail:border-t-0 @min-[1120px]/list-detail:border-l @min-[1120px]/list-detail:pt-0 @min-[1120px]/list-detail:pl-4"
              >
                {inspector}
              </aside>
            )}
          </div>
        </div>
      </div>
    </div>
  )
}
