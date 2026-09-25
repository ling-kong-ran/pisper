// Suggestions belong to the optional tools tray, not the uncluttered welcome canvas.
import {
  Bug,
  Code2,
  FileText,
  FlaskConical,
  Layers,
  ListChecks,
  SearchCheck,
  Wand2,
} from 'lucide-react'
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
  return (
    <>
      <details className="group relative text-xs text-muted-foreground">
        <summary className="cursor-pointer list-none rounded-lg px-3 py-2 transition-colors hover:bg-muted hover:text-foreground">
          {t('chat:focusSession.promptIdeas')}
        </summary>
        <div className="welcome-chips flex max-w-[560px] flex-wrap justify-center gap-2 pt-2">
          {welcomeChips(t, plansAvailable).map((chip) => (
            <button
              type="button"
              key={chip.label}
              data-target-cursor
              onClick={() => onPromptSelect(chip.prompt)}
              className="inline-flex min-h-9 items-center gap-2 rounded-full border border-border bg-background px-3 text-xs text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
            >
              <chip.icon size={14} className="text-muted-foreground" />
              {chip.label}
            </button>
          ))}
        </div>
      </details>
    </>
  )
}
