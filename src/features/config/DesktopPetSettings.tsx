// 桌面宠物设置：启用/透明度/宠物选择与商店浏览。
import { useCallback, useEffect, useMemo, useState } from 'react'
import { Cat, Download, ExternalLink, RefreshCw, Search } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import {
  SettingsBadge as Badge,
  SettingsCard as Panel,
  SettingsSwitch as Toggle,
} from './settings-primitives'
import { Slider } from '@/components/ui/slider'
import { apiJson } from '@/lib/api'
import type { Notify } from '@/app/route-context'
import type { DesktopPetCatalogItem, DesktopPetStatus } from '@/types/update'

import { Button } from '@/components/ui/button'

import { AppNotice } from '@/components/ui/app-primitives'

type PetController = {
  getStatus: () => Promise<DesktopPetStatus>
  setEnabled: (enabled: boolean) => Promise<DesktopPetStatus>
  setOpacity: (opacity: number) => Promise<DesktopPetStatus>
  search: (query: string) => Promise<DesktopPetCatalogItem[]>
  install: (slug: string) => Promise<DesktopPetStatus>
  select: (slug: string) => Promise<DesktopPetStatus>
  openCatalog: () => void
}

function announcePetChange(status: DesktopPetStatus) {
  window.dispatchEvent(new CustomEvent('pisper:desktop-pet-changed', { detail: status }))
}

export function DesktopPetSettings({ notify }: { notify: Notify }) {
  const { t } = useI18n()
  const bridge = window.pisperDesktop
  const controller = useMemo<PetController>(() => {
    if (bridge?.getPetStatus) {
      return {
        getStatus: () => bridge.getPetStatus!(),
        setEnabled: (enabled) => bridge.setPetEnabled!(enabled),
        setOpacity: (opacity) => bridge.setPetOpacity!(opacity),
        search: (query) => bridge.searchPets!(query),
        install: (nextSlug) => bridge.installPet!(nextSlug),
        select: (nextSlug) => bridge.selectPet!(nextSlug),
        openCatalog: () => void bridge.openPetdex?.(),
      }
    }
    return {
      getStatus: () => apiJson<DesktopPetStatus>('/api/desktop-pet'),
      setEnabled: (enabled) =>
        apiJson<DesktopPetStatus>('/api/desktop-pet/enabled', {
          method: 'POST',
          body: { enabled },
        }),
      setOpacity: (opacity) =>
        apiJson<DesktopPetStatus>('/api/desktop-pet/opacity', {
          method: 'POST',
          body: { opacity },
        }),
      search: (query) =>
        apiJson<DesktopPetCatalogItem[]>(
          `/api/desktop-pet/catalog?query=${encodeURIComponent(query)}`,
        ),
      install: (nextSlug) =>
        apiJson<DesktopPetStatus>('/api/desktop-pet/install', {
          method: 'POST',
          body: { slug: nextSlug },
        }),
      select: (nextSlug) =>
        apiJson<DesktopPetStatus>('/api/desktop-pet/select', {
          method: 'POST',
          body: { slug: nextSlug },
        }),
      openCatalog: () => window.open('https://petdex.dev', '_blank', 'noopener,noreferrer'),
    }
  }, [bridge])
  const [status, setStatus] = useState<DesktopPetStatus | null>(null)
  const [slug, setSlug] = useState('')
  const [catalog, setCatalog] = useState<DesktopPetCatalogItem[]>([])
  const [busy, setBusy] = useState('')
  const [error, setError] = useState('')

  const load = useCallback(async () => {
    try {
      setError('')
      setStatus(await controller.getStatus())
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught))
    }
  }, [controller])

  const searchCatalog = useCallback(async () => {
    setBusy('search')
    try {
      setError('')
      setCatalog(await controller.search(slug))
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught))
    } finally {
      setBusy('')
    }
  }, [controller, slug])

  useEffect(() => {
    void load()
    void controller
      .search('')
      .then(setCatalog)
      .catch(() => {})
  }, [controller, load])

  const toggle = async (enabled: boolean) => {
    setBusy('toggle')
    try {
      setError('')
      const next = await controller.setEnabled(enabled)
      setStatus(next)
      announcePetChange(next)
      notify(
        enabled
          ? t('config:desktopPetSettings.petEnabled')
          : t('config:desktopPetSettings.petDisabled'),
      )
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught))
    } finally {
      setBusy('')
    }
  }

  const changeOpacity = async (opacity: number) => {
    setBusy('opacity')
    try {
      setError('')
      const next = await controller.setOpacity(opacity)
      setStatus(next)
      announcePetChange(next)
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught))
    } finally {
      setBusy('')
    }
  }

  const install = async (requestedSlug = slug) => {
    if (!requestedSlug.trim()) return
    setBusy('install')
    try {
      setError('')
      const next = await controller.install(requestedSlug)
      setStatus(next)
      announcePetChange(next)
      notify(
        t('config:desktopPetSettings.petInstalled', {
          name: next.selectedName || requestedSlug,
        }),
      )
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught))
    } finally {
      setBusy('')
    }
  }

  const select = async (nextSlug: string) => {
    if (nextSlug === status?.selectedSlug) return
    setBusy(`select:${nextSlug}`)
    try {
      setError('')
      const next = await controller.select(nextSlug)
      setStatus(next)
      announcePetChange(next)
      notify(t('config:desktopPetSettings.petSelected', { name: next.selectedName || nextSlug }))
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught))
    } finally {
      setBusy('')
    }
  }

  return (
    <div className="flex w-[min(100%,_760px)] flex-col gap-[12px] desktop-pet-settings">
      <Panel className="[padding:18px]">
        <div className="language-settings-heading flex items-start gap-[11px] [&_h2]:text-[16px] [&_p]:mt-[4px] [&_p]:text-[var(--text-muted)] [&_p]:text-[13px] [&_p]:leading-[1.55]">
          <span className="grid w-[38px] h-[38px] [flex:0_0_auto] place-items-center rounded-[11px] bg-[var(--star-soft)] text-[var(--star-strong)]">
            <Cat size={19} />
          </span>
          <div>
            <h2>{t('config:desktopPetSettings.title')}</h2>
            <p>{t('config:desktopPetSettings.description')}</p>
          </div>
        </div>

        {!status ? (
          <AppNotice className="[margin-top:15px]">
            <RefreshCw className="animate-spin" size={16} />
            <span>{t('config:desktopPetSettings.loading')}</span>
          </AppNotice>
        ) : (
          <div className="notification-option [&_>_div]:flex [&_>_div]:min-w-0 [&_>_div]:flex-col [&_>_div]:gap-[4px] [&_strong]:text-[13px] [&_small]:text-[var(--text-muted)] [&_small]:text-[12px] [&_small]:leading-[1.45] grid grid-cols-[auto_minmax(0,1fr)_auto_auto] items-center gap-[11px] mt-4">
            <Cat size={18} />
            <div>
              <strong>{t('config:desktopPetSettings.showOnDesktop')}</strong>
              <small>
                {status.selectedName
                  ? t('config:desktopPetSettings.currentPet', { name: status.selectedName })
                  : t('config:desktopPetSettings.installBeforeEnabling')}
              </small>
            </div>
            <Badge tone={status.running ? 'green' : 'gray'}>
              {status.running
                ? t('config:desktopPetSettings.running')
                : t('config:desktopPetSettings.stopped')}
            </Badge>
            <Toggle
              value={status.enabled}
              disabled={busy === 'toggle' || !status.installed.length}
              onChange={toggle}
              ariaLabel={t('config:desktopPetSettings.showOnDesktop')}
            />
          </div>
        )}

        {status && (
          <div className="desktop-pet-opacity [&_>_div]:flex [&_>_div]:items-center [&_>_div]:justify-between [&_>_div]:gap-[8px] [&_>_div]:text-[12px] [&_>_div_span]:text-[var(--text-muted)] [&_>_div_span]:[font-variant-numeric:tabular-nums] grid grid-cols-[150px_minmax(0,1fr)] items-center gap-[14px] [border-top:1px_solid_var(--stroke-soft)] [padding-top:14px] mt-4">
            <div>
              <strong>{t('config:desktopPetSettings.opacity')}</strong>
              <span>{Math.round((status.opacity ?? 1) * 100)}%</span>
            </div>
            <Slider
              value={[Math.round((status.opacity ?? 1) * 100)]}
              min={20}
              max={100}
              step={5}
              disabled={busy === 'opacity'}
              onValueCommit={([value]) => void changeOpacity(value / 100)}
              aria-label={t('config:desktopPetSettings.opacity')}
            />
          </div>
        )}

        {error && <div className="text-[var(--danger)] text-[13px] mt-3">{error}</div>}
      </Panel>

      <Panel className="[padding:18px]">
        <div className="language-settings-heading flex items-start gap-[11px] [&_h2]:text-[16px] [&_p]:mt-[4px] [&_p]:text-[var(--text-muted)] [&_p]:text-[13px] [&_p]:leading-[1.55]">
          <span className="grid w-[38px] h-[38px] [flex:0_0_auto] place-items-center rounded-[11px] bg-[var(--star-soft)] text-[var(--star-strong)]">
            <Download size={19} />
          </span>
          <div>
            <h2>{t('config:desktopPetSettings.installTitle')}</h2>
            <p>{t('config:desktopPetSettings.installDescription')}</p>
          </div>
        </div>
        <div className="workspace-path-form grid h-[39px] grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-[8px] mt-[16px] [border:1px_solid_var(--stroke)] rounded-[var(--r-sm)] p-[3px_3px_3px_10px] text-[var(--text-muted)] [&_input]:min-w-0 [&_input]:border-0 [&_input]:[outline:0] [&_input]:font-[ui-monospace,_SFMono-Regular,_Consolas,_'Liberation_Mono',_monospace] [&_input]:text-[12px] dark:[&_input]:bg-[var(--solid)] dark:[&_input]:text-[var(--text)] desktop-pet-search !grid-cols-[minmax(0,1fr)_auto] mt-4">
          <input
            value={slug}
            onChange={(event) => setSlug(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === 'Enter') void searchCatalog()
            }}
            placeholder={t('config:desktopPetSettings.searchPlaceholder')}
            aria-label={t('config:desktopPetSettings.petSlug')}
          />
          <Button
            size="lg"
            className="min-w-[104px]"
            disabled={busy === 'search'}
            onClick={searchCatalog}
          >
            {busy === 'search' ? (
              <RefreshCw className="animate-spin" size={14} />
            ) : (
              <Search size={14} />
            )}
            {t('config:desktopPetSettings.search')}
          </Button>
        </div>
        {catalog.length > 0 && (
          <div className="language-choice-grid max-[650px]:grid-cols-[1fr] grid grid-cols-[repeat(2,_minmax(0,_1fr))] gap-[9px] [margin-top:18px] mt-4">
            {catalog.map((pet) => {
              const installed = status?.installed.some((item) => item.slug === pet.slug)
              return (
                <button
                  type="button"
                  className="language-choice hover:border-[var(--star)] hover:bg-[var(--accent-soft)] hover:[transform:translateY(-1px)] [&.selected]:border-[var(--star)] [&.selected]:bg-[var(--star-soft)] [&.selected]:shadow-[0_0_0_3px_var(--accent-ring)] grid min-h-[80px] grid-cols-[auto_minmax(0,_1fr)_auto] items-center gap-[10px] [border:1px_solid_var(--stroke)] rounded-[var(--r-sm)] bg-[var(--surface-subtle)] [padding:11px] text-[var(--text)] text-left [transition:border-color_var(--d1)_var(--ease-out),_background_var(--d1)_var(--ease-out),_box-shadow_var(--d1)_var(--ease-out),_transform_var(--d1)_var(--ease-out)]"
                  disabled={busy === 'install'}
                  onClick={() => (installed ? select(pet.slug) : install(pet.slug))}
                  key={pet.slug}
                >
                  <span className="language-choice-mark grid w-[34px] h-[34px] place-items-center rounded-[9px] bg-[var(--solid)] text-[var(--star-strong)] text-[12px] font-[800] tracking-[.03em]">
                    <Cat size={16} />
                  </span>
                  <span className="language-choice-copy [&_strong]:text-[13px] [&_small]:text-[var(--text-muted)] [&_small]:text-[12px] flex min-w-0 flex-col gap-[3px]">
                    <strong>{pet.displayName}</strong>
                    <small>{pet.slug}</small>
                  </span>
                  <Badge tone={installed ? 'green' : 'blue'}>
                    {installed
                      ? t('config:desktopPetSettings.installed')
                      : t('config:desktopPetSettings.install')}
                  </Badge>
                </button>
              )
            })}
          </div>
        )}
        <div className="mt-[15px] flex gap-2 max-[650px]:flex-wrap">
          <Button
            variant="outline"
            size="lg"
            className="bg-surface-subtle"
            onClick={controller.openCatalog}
          >
            <ExternalLink size={14} />
            {t('config:desktopPetSettings.browsePetdex')}
          </Button>
        </div>
      </Panel>

      {status && status.installed.length > 0 && (
        <Panel className="[padding:18px]">
          <div className="language-settings-heading flex items-start gap-[11px] [&_h2]:text-[16px] [&_p]:mt-[4px] [&_p]:text-[var(--text-muted)] [&_p]:text-[13px] [&_p]:leading-[1.55]">
            <span className="grid w-[38px] h-[38px] [flex:0_0_auto] place-items-center rounded-[11px] bg-[var(--star-soft)] text-[var(--star-strong)]">
              <Cat size={19} />
            </span>
            <div>
              <h2>{t('config:desktopPetSettings.installedTitle')}</h2>
              <p>{t('config:desktopPetSettings.installedDescription')}</p>
            </div>
          </div>
          <div
            className="language-choice-grid max-[650px]:grid-cols-[1fr] grid grid-cols-[repeat(2,_minmax(0,_1fr))] gap-[9px] [margin-top:18px] mt-4"
            role="radiogroup"
          >
            {status.installed.map((pet) => {
              const selected = pet.slug === status.selectedSlug
              return (
                <button
                  type="button"
                  className={`language-choice hover:border-[var(--star)] hover:bg-[var(--accent-soft)] hover:[transform:translateY(-1px)] [&.selected]:border-[var(--star)] [&.selected]:bg-[var(--star-soft)] [&.selected]:shadow-[0_0_0_3px_var(--accent-ring)] grid min-h-[80px] grid-cols-[auto_minmax(0,_1fr)_auto] items-center gap-[10px] [border:1px_solid_var(--stroke)] rounded-[var(--r-sm)] bg-[var(--surface-subtle)] [padding:11px] text-[var(--text)] text-left [transition:border-color_var(--d1)_var(--ease-out),_background_var(--d1)_var(--ease-out),_box-shadow_var(--d1)_var(--ease-out),_transform_var(--d1)_var(--ease-out)] ${selected ? 'selected' : ''}`}
                  role="radio"
                  aria-checked={selected}
                  disabled={busy === `select:${pet.slug}`}
                  onClick={() => select(pet.slug)}
                  key={`${pet.source}:${pet.slug}`}
                >
                  <span className="language-choice-mark grid w-[34px] h-[34px] place-items-center rounded-[9px] bg-[var(--solid)] text-[var(--star-strong)] text-[12px] font-[800] tracking-[.03em]">
                    <Cat size={16} />
                  </span>
                  <span className="language-choice-copy [&_strong]:text-[13px] [&_small]:text-[var(--text-muted)] [&_small]:text-[12px] flex min-w-0 flex-col gap-[3px]">
                    <strong>{pet.name}</strong>
                    <small>{pet.slug}</small>
                  </span>
                  <Badge tone={pet.source === 'pisper' ? 'blue' : 'gray'}>
                    {pet.source === 'pisper' ? 'Pisper' : 'Petdex'}
                  </Badge>
                </button>
              )
            })}
          </div>
        </Panel>
      )}
    </div>
  )
}
