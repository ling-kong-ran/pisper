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
    <section aria-label={t('chat:focusSession.promptIdeas')} className="pt-5 text-left text-sm">
      <div className="mb-3 flex items-center justify-between gap-2">
        <h2 className="text-sm font-medium">{t('chat:focusSession.promptIdeas')}</h2>
        <div className="flex gap-1">
          <Button
            type="button"
            variant="ghost"
            size="icon"
            className="size-9 rounded-full max-[650px]:size-11"
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
            className="size-9 rounded-full max-[650px]:size-11"
            aria-label={t('chat:workbench.nextIdeas')}
            disabled={currentPage === lastPage}
            onClick={() => setPage(currentPage + 1)}
          >
            <ChevronRight />
          </Button>
        </div>
      </div>
      <div className="welcome-chips grid grid-cols-3 gap-3 @max-[600px]:grid-cols-1">
        {chips.slice(currentPage * 3, currentPage * 3 + 3).map((chip) => (
          <button
            type="button"
            key={chip.label}
            aria-label={chip.label}
            data-target-cursor
            onClick={() => onPromptSelect(chip.prompt)}
            className="flex min-h-32 min-w-0 flex-col gap-3 rounded-2xl border border-border bg-background p-3 text-left font-normal shadow-sm transition-colors hover:bg-muted focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring @max-[600px]:min-h-0 @max-[600px]:w-full"
          >
            <span className="flex w-full items-center justify-between gap-2">
              <chip.icon size={20} aria-hidden="true" />
              <span className="rounded-full border border-border px-2 py-1 text-xs">
                {t('chat:workbench.tryPrompt')}
              </span>
            </span>
            <span>{chip.label}</span>
            <span className="text-xs leading-relaxed text-muted-foreground">
              {chip.prompt.replace(/[：:]$/, '')}
            </span>
          </button>
        ))}
      </div>
    </section>
  )
}
