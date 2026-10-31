// 共享页头：48px 面包屑顶栏（所属入口 / 当前页）+ 终端开关。
// A6 起逐批退役：功能页（src/components/app/FeaturePage.tsx，FEATURE_TEMPLATE_PAGES）与设置分区（A6 第 4 批，
// 同一模板）都自带页头，现在只剩组件工作室与“我的组件”整页在用，A7 重做组件时一并退役。
// 页面搜索与主操作都已挪进各页自己的工具行，这里不再有搜索框与主操作注册表的按钮。
// 主题切换不在页头（规格 v2 第 2.5 节）：设置 › 界面与 ⌘K“切换主题”。
// 从“功能”进入时面包屑上一级是可点的“‹ 功能”；手机是标题左侧 44px 的文字返回键（iOS 没有返回键）。
// 没有应用标题栏了：Windows 自绘标题栏时页头空白处是拖拽区，右侧预留 144px 给窗口按钮（macOS / Linux 只留 12px）。
import type { RefObject } from 'react'
import { ChevronLeft, TerminalSquare } from 'lucide-react'
import { MORE_PAGES } from '@/app/more-pages'
import { useI18n } from '@/app/use-i18n'
import { cn } from '@/lib/utils'
import { useShortcutLabel } from '@/lib/shortcuts'

import { Button } from '@/components/ui/button'

type PageHeaderProps = {
  elementRef?: RefObject<HTMLElement | null>
  meta: readonly [string, string]
  page: string
  desktopPlatform: string
  terminalOpen: boolean
  onToggleTerminal: () => void
  // 当前页属于“功能”时提供，渲染“‹ 功能”返回。
  onBackToMore?: () => void
  // 手机外壳（规格 7.1）：页名在外壳顶栏里，这里的标题只给读屏；“‹ 功能”返回保留（iOS 没有返回键）。
  phoneShell?: boolean
}

export function PageHeader({
  elementRef,
  meta,
  page,
  desktopPlatform,
  terminalOpen,
  onToggleTerminal,
  onBackToMore,
  phoneShell = false,
}: PageHeaderProps) {
  const { t } = useI18n()
  const terminalShortcut = useShortcutLabel('toggleTerminal')
  const desktop = Boolean(desktopPlatform)
  // 面包屑上一级：功能页归「功能」（从功能进入时可点返回）；一级页没有上一级。
  const parent = MORE_PAGES.has(page) || onBackToMore ? t('navigation:sidebar.features') : ''

  return (
    <header
      ref={elementRef}
      data-focus-region="header"
      data-window-drag-region
      className={cn(
        '@container/page-header relative z-[2] flex flex-none flex-col',
        desktop && '[-webkit-app-region:drag]',
      )}
    >
      <div
        className={cn(
          'flex min-h-12 items-center gap-2 border-b border-stroke-soft py-1.5 pr-2.5 in-data-[density=compact]:min-h-11 max-[650px]:min-h-0 max-[650px]:flex-wrap max-[650px]:gap-x-2 max-[650px]:gap-y-0 max-[650px]:border-b-0 max-[650px]:px-4 max-[650px]:pt-0 max-[650px]:pb-3',
          'pl-[calc(16px+var(--workbench-leading-inset,0px))] max-[650px]:pl-4',
          // Windows 自绘标题栏：右上角 144px 是窗口按钮。
          '[[data-custom-titlebar]_&]:pr-[144px]',
        )}
      >
        <div
          className={cn(
            'mr-auto flex min-w-0 items-center gap-1.5 text-[13px]',
            // 手机：56px 行，左对齐大标题。
            'max-[650px]:h-14 max-[650px]:basis-full max-[650px]:gap-2',
            // 手机外壳：标题在顶栏里，这一行只剩“‹ 功能”（44px）；没有返回时整行只留给读屏的标题。
            phoneShell && (parent && onBackToMore ? 'max-[650px]:h-11' : 'max-[650px]:h-0'),
          )}
        >
          {parent && onBackToMore ? (
            <>
              {/* 桌面是面包屑里的“‹ 功能”；手机是标题左侧 44px 高的文字返回键（规格 2.1）。 */}
              <button
                type="button"
                data-back-to-more
                className="-ml-1.5 flex h-7 shrink-0 items-center gap-0.5 rounded-sm pr-1.5 pl-1 text-content-muted transition-colors duration-(--d-fast) hover:bg-surface-hover hover:text-content focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-focus max-[650px]:-ml-3 max-[650px]:h-11 max-[650px]:min-w-11 max-[650px]:pr-2 max-[650px]:text-sm max-[650px]:text-content-secondary"
                aria-label={t('navigation:shell.backToFeaturesLabel')}
                onClick={onBackToMore}
              >
                <ChevronLeft className="size-4 max-[650px]:size-5" aria-hidden="true" />
                <span>{parent}</span>
              </button>
              <span aria-hidden="true" className="shrink-0 text-content-faint max-[650px]:hidden">
                /
              </span>
            </>
          ) : (
            parent && (
              <>
                <span className="shrink-0 text-content-muted max-[900px]:hidden">{parent}</span>
                <span aria-hidden="true" className="shrink-0 text-content-faint max-[900px]:hidden">
                  /
                </span>
              </>
            )
          )}
          <h1
            className={cn(
              'max-w-full min-w-[4em] truncate text-[13px] leading-5 font-medium text-content max-[650px]:min-w-0 max-[650px]:text-2xl max-[650px]:leading-8 max-[650px]:font-[650] max-[650px]:tracking-[-.015em]',
              phoneShell && 'sr-only',
            )}
          >
            {meta[0]}
          </h1>
          {/* 页面说明：桌面跟在标题后作次要文字，窄于 900px 时收起。 */}
          {meta[1] && !phoneShell && (
            <p className="ml-1.5 min-w-0 truncate text-xs text-content-muted max-[900px]:hidden max-[650px]:ml-0 max-[650px]:block max-[650px]:flex-1 max-[650px]:basis-0">
              {meta[1]}
            </p>
          )}
        </div>
        {desktop && (
          <div className="flex min-w-0 items-center justify-end gap-1.5 [-webkit-app-region:no-drag]">
            <Button
              variant="ghost"
              size="icon"
              className={cn(
                'text-content-muted max-[650px]:hidden',
                terminalOpen && 'bg-surface-hover text-accent-fg',
              )}
              title={
                terminalShortcut
                  ? `${t('navigation:pageHeader.toggleTerminal')} (${terminalShortcut})`
                  : t('navigation:pageHeader.toggleTerminal')
              }
              aria-label={t('navigation:pageHeader.toggleTerminal')}
              aria-pressed={terminalOpen}
              onClick={onToggleTerminal}
            >
              <TerminalSquare size={16} />
            </Button>
          </div>
        )}
      </div>
    </header>
  )
}
