// 关于页集中展示版本、项目链接与许可证，并把社区支持入口放在用户主动查看的位置。
import { Code2, ExternalLink, Globe2, Scale, Star, type LucideIcon } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { Button } from '@/components/ui/button'
import type { AppUpdateController } from '@/types/update'
import {
  SettingsBadge as Badge,
  SettingsCard as Panel,
  SettingsSectionTitle as SectionTitle,
} from './settings-primitives'

const BUILD_VERSION = import.meta.env.VITE_APP_VERSION || '0.0.0'
const PROJECT_URL = 'https://ling-kong-ran.github.io/pisper/'
const REPOSITORY_URL = 'https://github.com/ling-kong-ran/pisper'
const LICENSE_URL = `${REPOSITORY_URL}/blob/release/LICENSE`

function AboutLink({
  href,
  icon: Icon,
  label,
  value,
}: {
  href: string
  icon: LucideIcon
  label: string
  value: string
}) {
  return (
    <a
      className="group grid min-h-14 grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-3 py-3 text-left no-underline"
      href={href}
      target="_blank"
      rel="noopener noreferrer"
    >
      <span className="grid size-8 place-items-center rounded-[var(--r-xs)] bg-[var(--surface-muted)] text-[var(--text-muted)] transition-colors group-hover:text-[var(--star-strong)]">
        <Icon size={16} />
      </span>
      <span className="min-w-0">
        <strong className="block text-[13px] text-[var(--text)]">{label}</strong>
        <small className="mt-0.5 block truncate text-[12px] text-[var(--text-muted)]">
          {value}
        </small>
      </span>
      <ExternalLink
        className="text-[var(--text-muted)] transition-colors group-hover:text-[var(--star-strong)]"
        size={14}
      />
    </a>
  )
}

export function AboutSettings({ update }: { update: AppUpdateController }) {
  const { t } = useI18n()
  const version = update.info?.version || BUILD_VERSION

  return (
    <div className="mx-auto flex w-full max-w-[760px] flex-col gap-3">
      <div className="flex items-center gap-4 px-1 py-3">
        <img
          className="size-16 shrink-0 rounded-[14px] shadow-[0_10px_28px_-14px_var(--shadow)]"
          src="/favicon.svg"
          alt="Pisper"
        />
        <div className="min-w-0">
          <div className="flex flex-wrap items-center gap-2">
            <h1 className="text-[22px] font-bold tracking-[0]">Pisper</h1>
            <Badge tone="gray">v{version}</Badge>
          </div>
          <p className="mt-1 text-[13px] leading-5 text-[var(--text-muted)]">
            {t('config:aboutSettings.description')}
          </p>
        </div>
      </div>

      <Panel className="p-5" data-config-card="about-project">
        <SectionTitle title={t('config:aboutSettings.projectInformation')} />
        <div className="mt-3 divide-y divide-[var(--stroke-soft)] border-y border-[var(--stroke-soft)]">
          <AboutLink
            href={PROJECT_URL}
            icon={Globe2}
            label={t('config:aboutSettings.website')}
            value="ling-kong-ran.github.io/pisper"
          />
          <AboutLink
            href={REPOSITORY_URL}
            icon={Code2}
            label={t('config:aboutSettings.sourceCode')}
            value="github.com/ling-kong-ran/pisper"
          />
          <AboutLink
            href={LICENSE_URL}
            icon={Scale}
            label={t('config:aboutSettings.license')}
            value="MIT License"
          />
        </div>
      </Panel>

      <Panel className="p-5" data-config-card="about-support">
        <div className="flex flex-wrap items-center justify-between gap-4 max-[540px]:flex-col max-[540px]:items-stretch">
          <div className="min-w-0 flex-1">
            <SectionTitle title={t('config:aboutSettings.supportPisper')} />
            <p className="mt-1 max-w-[520px] text-[13px] leading-5 text-[var(--text-muted)]">
              {t('config:aboutSettings.supportDescription')}
            </p>
          </div>
          <Button asChild size="lg" className="max-[540px]:w-full">
            <a href={REPOSITORY_URL} target="_blank" rel="noopener noreferrer">
              <Star size={15} />
              {t('config:aboutSettings.starOnGitHub')}
            </a>
          </Button>
        </div>
      </Panel>

      <p className="px-1 py-2 text-center text-[12px] text-[var(--text-muted)]">
        {t('config:aboutSettings.copyright')}
      </p>
    </div>
  )
}
