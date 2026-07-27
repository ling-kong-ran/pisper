import { useCallback, useEffect, useMemo, useState } from 'react'
import { Cat, Download, ExternalLink, RefreshCw, Search } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { Badge, Panel, Toggle } from '@/components/ui'
import { Slider } from '@/components/ui/slider'
import { apiJson } from '@/lib/api'
import type { Notify } from '@/app/route-context'
import type { DesktopPetCatalogItem, DesktopPetStatus } from '@/types/update'

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
    <div className="language-settings desktop-pet-settings">
      <Panel className="language-settings-card">
        <div className="language-settings-heading">
          <span className="language-settings-icon">
            <Cat size={19} />
          </span>
          <div>
            <h2>{t('config:desktopPetSettings.title')}</h2>
            <p>{t('config:desktopPetSettings.description')}</p>
          </div>
        </div>

        {!status ? (
          <div className="permission-note language-settings-note">
            <RefreshCw className="spin" size={16} />
            <span>{t('config:desktopPetSettings.loading')}</span>
          </div>
        ) : (
          <div className="notification-option mt-4">
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
          <div className="desktop-pet-opacity mt-4">
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

        {error && <div className="attachment-error mt-3">{error}</div>}
      </Panel>

      <Panel className="language-settings-card">
        <div className="language-settings-heading">
          <span className="language-settings-icon">
            <Download size={19} />
          </span>
          <div>
            <h2>{t('config:desktopPetSettings.installTitle')}</h2>
            <p>{t('config:desktopPetSettings.installDescription')}</p>
          </div>
        </div>
        <div className="workspace-path-form desktop-pet-search mt-4">
          <input
            value={slug}
            onChange={(event) => setSlug(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === 'Enter') void searchCatalog()
            }}
            placeholder={t('config:desktopPetSettings.searchPlaceholder')}
            aria-label={t('config:desktopPetSettings.petSlug')}
          />
          <button className="button primary" disabled={busy === 'search'} onClick={searchCatalog}>
            {busy === 'search' ? <RefreshCw className="spin" size={14} /> : <Search size={14} />}
            {t('config:desktopPetSettings.search')}
          </button>
        </div>
        {catalog.length > 0 && (
          <div className="language-choice-grid mt-4">
            {catalog.map((pet) => {
              const installed = status?.installed.some((item) => item.slug === pet.slug)
              return (
                <button
                  type="button"
                  className="language-choice"
                  disabled={busy === 'install'}
                  onClick={() => (installed ? select(pet.slug) : install(pet.slug))}
                  key={pet.slug}
                >
                  <span className="language-choice-mark">
                    <Cat size={16} />
                  </span>
                  <span className="language-choice-copy">
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
        <div className="button-row">
          <button className="button secondary" onClick={controller.openCatalog}>
            <ExternalLink size={14} />
            {t('config:desktopPetSettings.browsePetdex')}
          </button>
        </div>
      </Panel>

      {status && status.installed.length > 0 && (
        <Panel className="language-settings-card">
          <div className="language-settings-heading">
            <span className="language-settings-icon">
              <Cat size={19} />
            </span>
            <div>
              <h2>{t('config:desktopPetSettings.installedTitle')}</h2>
              <p>{t('config:desktopPetSettings.installedDescription')}</p>
            </div>
          </div>
          <div className="language-choice-grid mt-4" role="radiogroup">
            {status.installed.map((pet) => {
              const selected = pet.slug === status.selectedSlug
              return (
                <button
                  type="button"
                  className={`language-choice ${selected ? 'selected' : ''}`}
                  role="radio"
                  aria-checked={selected}
                  disabled={busy === `select:${pet.slug}`}
                  onClick={() => select(pet.slug)}
                  key={`${pet.source}:${pet.slug}`}
                >
                  <span className="language-choice-mark">
                    <Cat size={16} />
                  </span>
                  <span className="language-choice-copy">
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
