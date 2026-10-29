// 新建页推荐任务直接展示在输入框下方；选择仅填入草稿，不自动提交。
import {
  Bug,
  ChevronLeft,
  ChevronRight,
  Code2,
  FileText,
  FlaskConical,
  Layers,
  ListChecks,
  SearchCheck,
  Wand2,
} from 'lucide-react'
import { useState } from 'react'
import { Button } from '@/components/ui/button'
import type { I18nValues } from '@/app/i18n'
import { useI18n } from '@/app/use-i18n'
type Translate = (message: string, values?: I18nValues) => string
function welcomeChips(t: Translate, plansAvailable: boolean) {
  return [
    {
      icon: Code2,
      label: t('chat:focusSession.explainCode'),
      prompt: t('chat:focusSession.explainHowThisCodeWorks'),
    },
    {
      icon: FlaskConical,
      label: t('chat:focusSession.writeTests'),
      prompt: t('chat:focusSession.writeUnitTestsForTheFollowingCode'),
    },
    {
      icon: Wand2,
      label: t('chat:focusSession.refactor'),
      prompt: t('chat:focusSession.refactorThisCodeAndExplainTheImprovements'),
    },
    {
      icon: Bug,
      label: t('chat:focusSession.findABug'),
      prompt: t('chat:focusSession.helpMeLocateAndFixThisBug'),
    },
    {
      icon: FileText,
      label: t('chat:focusSession.summarize'),
      prompt: t('chat:focusSession.summarizeContentAndExtractKeyPoints'),
    },
    ...(plansAvailable
      ? [
          {
            icon: ListChecks,
            label: t('chat:focusSession.makeAPlan'),
            prompt: t('chat:focusSession.makeAClearActionablePlanForThisGoal'),
          },
        ]
      : []),
    {
      icon: Layers,
      label: t('chat:focusSession.organizeInformation'),
      prompt: t('chat:focusSession.organizeThisInformationIntoAClearStructure'),
    },
    {
      icon: SearchCheck,
      label: t('chat:focusSession.findIssues'),
      prompt: t('chat:focusSession.reviewThisContentAndSuggestImprovements'),
    },
  ]
}

export default function QuickPromptIdeas({
  plansAvailable,
  onPromptSelect,
}: {
  plansAvailable: boolean
  onPromptSelect: (prompt: string) => void
}) {
  const { t } = useI18n()
  const [page, setPage] = useState(0)
  const chips = welcomeChips(t, plansAvailable)
  const lastPage = Math.ceil(chips.length / 3) - 1
  const currentPage = Math.min(page, lastPage)
  return (
    <section aria-label={t('chat:focusSession.promptIdeas')} className="pt-6 text-left text-sm">
      <div className="mb-2 flex min-h-7 items-center justify-between gap-2">
        <h2 className="text-[12px] font-medium tracking-[.02em] text-content-muted">
          {t('chat:focusSession.promptIdeas')}
        </h2>
        {/* 窄容器改为横滑浏览全部任务，翻页按钮只在宽布局出现。 */}
        <div className="flex gap-1 @max-[600px]:hidden">
          <Button
            type="button"
            variant="ghost"
            size="icon"
            aria-label={t('chat:workbench.previousIdeas')}
            disabled={currentPage === 0}
            onClick={() => setPage(currentPage - 1)}
          >
            <ChevronLeft />
          </Button>
          <Button
            type="button"
            variant="ghost"
            size="icon"
            aria-label={t('chat:workbench.nextIdeas')}
            disabled={currentPage === lastPage}
            onClick={() => setPage(currentPage + 1)}
          >
            <ChevronRight />
          </Button>
        </div>
      </div>
      <div className="welcome-chips grid grid-cols-3 gap-2 @max-[600px]:flex @max-[600px]:snap-x @max-[600px]:overflow-x-auto @max-[600px]:pb-2">
        {chips.map((chip, index) => (
          <button
            type="button"
            key={chip.label}
            aria-label={chip.label}
            data-off-page={Math.floor(index / 3) !== currentPage || undefined}
            data-target-cursor
            onClick={() => onPromptSelect(chip.prompt)}
            className="group/idea flex min-h-[112px] min-w-0 cursor-pointer flex-col gap-2 rounded-md border border-stroke bg-panel p-3 text-left font-normal transition-colors duration-(--d-fast) ease-precise hover:border-stroke-strong hover:bg-surface-subtle focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-focus @max-[600px]:w-[min(240px,80cqw)] @max-[600px]:shrink-0 @max-[600px]:snap-start @min-[600px]:data-off-page:hidden"
          >
            {/* 图标放进小键帽：与功能栏、品牌键同一「凸起键」语言；「试一试」只在悬停/聚焦时浮现。 */}
            <span className="flex w-full items-center justify-between gap-2">
              <span className="grid size-7 place-items-center rounded-sm border border-key-border bg-key text-content-secondary shadow-key">
                <chip.icon size={15} aria-hidden="true" />
              </span>
              <span className="rounded-xs border border-stroke px-1.5 text-[12px] leading-[18px] text-content-muted opacity-0 transition-opacity duration-(--d-fast) group-hover/idea:opacity-100 group-focus-visible/idea:opacity-100 pointer-coarse:opacity-100">
                {t('chat:workbench.tryPrompt')}
              </span>
            </span>
            <span className="mt-1 text-[13px] font-medium text-foreground">{chip.label}</span>
            <span className="text-[12px] leading-[1.5] text-content-muted">
              {chip.prompt.replace(/[：:]$/, '')}
            </span>
          </button>
        ))}
      </div>
    </section>
  )
}
