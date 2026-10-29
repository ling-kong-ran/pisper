// 关于页集中展示版本、项目链接与许可证，并把社区支持入口放在用户主动查看的位置。
import type { MouseEvent } from 'react'
import { Code2, ExternalLink, Globe2, Scale, Star, type LucideIcon } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import type { Notify } from '@/app/route-context'
import { Button } from '@/components/ui/button'
import { openMobileExternalLink } from '@/lib/mobile-external-link'
import type { AppUpdateController } from '@/types/update'
import { BrandLogo } from '@/components/BrandLogo'
import {
  SettingsBadge as Badge,
  SettingsCard as Panel,
  SettingsGroup,
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
  onClick,
}: {
  href: string
  icon: LucideIcon
  label: string
  value: string
  onClick: (event: MouseEvent<HTMLAnchorElement>) => void
}) {
  return (
    <a
      className="group grid min-h-14 grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-3 py-2.5 text-left no-underline"
      href={href}
      target="_blank"
      rel="noopener noreferrer"
      data-web-preview="external"
      onClick={onClick}
    >
      <span className="grid size-7.5 place-items-center rounded-md border border-key-border bg-key text-content-muted shadow-key transition-colors group-hover:text-content">
        <Icon size={16} aria-hidden="true" />
      </span>
      <span className="min-w-0">
        <span className="block text-sm font-medium text-content">{label}</span>
        <span className="mt-0.5 block truncate font-mono text-xs text-content-muted">{value}</span>
      </span>
      <ExternalLink
        aria-hidden="true"
        className="text-content-muted transition-colors group-hover:text-content"
        size={14}
      />
    </a>
  )
}

export function AboutSettings({ update, notify }: { update: AppUpdateController; notify: Notify }) {
  const { t } = useI18n()
  const version = update.info?.version || BUILD_VERSION
  const openLink = (event: MouseEvent<HTMLAnchorElement>) => {
    const pending = openMobileExternalLink(event, event.currentTarget.href)
    void pending?.catch(() => notify(t('config:aboutSettings.openLinkFailed'), 'error'))
  }

  return (
    <div className="mx-auto flex w-full flex-col gap-6">
      {/* 品牌时刻：单色四芒星键帽 + 名称与等宽版本号；页面语义标题仍是页头的「关于」。 */}
      <div className="flex items-center gap-4">
        <span aria-hidden="true" className="shrink-0 rounded-[15px] shadow-raise">
          <BrandLogo size={56} />
        </span>
        <div className="min-w-0">
          <div className="flex flex-wrap items-center gap-2">
            <h2 className="text-[length:calc(var(--ui-font-size)*10/7)] leading-[1.3] font-semibold tracking-[-.015em] text-content">
              Pisper
            </h2>
            <Badge tone="gray" className="font-mono">
              v{version}
            </Badge>
          </div>
          <p className="mt-1 text-sm text-content-muted">{t('config:aboutSettings.description')}</p>
        </div>
      </div>

      <section data-config-card="about-project">
        <SectionTitle
          title={t('config:aboutSettings.projectInformation')}
          className="mb-3 text-[15px] leading-6 font-medium text-content"
        />
        <SettingsGroup>
          <AboutLink
            href={PROJECT_URL}
            icon={Globe2}
            label={t('config:aboutSettings.website')}
            value="ling-kong-ran.github.io/pisper"
            onClick={openLink}
          />
          <AboutLink
            href={REPOSITORY_URL}
            icon={Code2}
            label={t('config:aboutSettings.sourceCode')}
            value="github.com/ling-kong-ran/pisper"
            onClick={openLink}
          />
          <AboutLink
            href={LICENSE_URL}
            icon={Scale}
            label={t('config:aboutSettings.license')}
            value="MIT License"
            onClick={openLink}
          />
        </SettingsGroup>
      </section>

      <Panel className="p-4 py-4" data-config-card="about-support">
        <div className="flex flex-wrap items-center justify-between gap-4 max-[540px]:flex-col max-[540px]:items-stretch">
          <div className="min-w-0 flex-1">
            <SectionTitle title={t('config:aboutSettings.supportPisper')} />
            <p className="mt-0.5 max-w-[520px] text-xs leading-5 text-content-muted">
              {t('config:aboutSettings.supportDescription')}
            </p>
          </div>
          <Button asChild className="max-[650px]:h-11 max-[540px]:w-full">
            <a
              href={REPOSITORY_URL}
              target="_blank"
              rel="noopener noreferrer"
              data-web-preview="external"
              onClick={openLink}
            >
              <Star size={15} />
              {t('config:aboutSettings.starOnGitHub')}
            </a>
          </Button>
        </div>
      </Panel>

      <p className="text-center text-xs text-content-muted">
        {t('config:aboutSettings.copyright')}
      </p>
    </div>
  )
}
