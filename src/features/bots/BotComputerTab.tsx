// 设置里的「电脑」分区（B6，设计稿 botim-computer）：Bot 在哪台电脑、哪个目录干活，以及有哪些电脑可选。
// 换电脑：选电脑、填那台电脑上的目录（本机可以用目录选择器），保存后下一个任务起生效。
// 添加电脑：就地显示一次性配对码与要在那台电脑上运行的两条命令；不弹窗。
// 移除电脑：就地确认；还有 Bot 在用时由 Runtime 拒绝并说明是哪些 Bot。
import { useEffect, useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { Copy, Monitor, Plus } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { AppSelect } from '@/components/AppSelect'
import { WorkspacePicker } from '@/components/WorkspacePicker'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { cn } from '@/lib/utils'
import {
  createEnvironmentPairing,
  removeEnvironment,
  updateBot,
  type Bot,
  type BotEnvironment,
  type BotRemoteInfo,
  type BotTask,
} from './bots-api'
import { environmentLabel, executorCommands } from './bot-environments'
import { useBotsStore } from './bots-store'

type Pairing = { code: string; expiresAt: string; remote: BotRemoteInfo }

function EnvironmentRow({
  environment,
  onRemoved,
  onError,
}: {
  environment: BotEnvironment
  onRemoved: () => void
  onError: (message: string) => void
}) {
  const { t } = useI18n()
  const [confirming, setConfirming] = useState(false)
  const [busy, setBusy] = useState(false)
  const remove = async () => {
    setBusy(true)
    try {
      await removeEnvironment(environment.id)
      onRemoved()
    } catch (caught) {
      onError(caught instanceof Error ? caught.message : String(caught))
      setConfirming(false)
    } finally {
      setBusy(false)
    }
  }
  return (
    <li data-environment={environment.id} className="flex flex-col gap-1 py-2">
      <div className="flex items-center gap-2">
        <span
          aria-hidden="true"
          className={cn(
            'size-2 flex-none rounded-full',
            environment.online ? 'bg-success' : 'bg-content-faint',
          )}
        />
        <span className="min-w-0 flex-1 truncate text-sm font-medium text-content">
          {environmentLabel(t, environment)}
        </span>
        <span className="text-xs text-content-muted">
          {environment.online ? t('bots:computer.online') : t('bots:computer.offline')}
        </span>
        {environment.kind === 'node' &&
          (confirming ? (
            <>
              <Button
                size="sm"
                variant="ghost"
                disabled={busy}
                onClick={() => setConfirming(false)}
              >
                {t('bots:computer.cancel')}
              </Button>
              <Button size="sm" variant="destructive" disabled={busy} onClick={() => void remove()}>
                {t('bots:computer.confirmRemove')}
              </Button>
            </>
          ) : (
            <Button size="sm" variant="ghost" onClick={() => setConfirming(true)}>
              {t('bots:computer.remove')}
            </Button>
          ))}
      </div>
      {environment.roots.length > 0 && (
        <p className="truncate pl-4 font-mono text-xs text-content-muted">
          {environment.roots.join(' · ')}
        </p>
      )}
      {environment.usedBy.length > 0 && (
        <p className="pl-4 text-xs text-content-muted">
          {t('bots:computer.usedBy', {
            names: environment.usedBy.map((bot) => bot.name).join('、'),
          })}
        </p>
      )}
    </li>
  )
}

function PairingPanel({ pairing, onDone }: { pairing: Pairing; onDone: () => void }) {
  const { t } = useI18n()
  const navigate = useNavigate()
  const [copied, setCopied] = useState(false)
  const commands = executorCommands(pairing, {
    brain: t('bots:computer.brainPlaceholder'),
    fingerprint: 'SHA256:…',
    root: t('bots:computer.rootPlaceholder'),
  })
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(commands.join('\n'))
      setCopied(true)
    } catch {
      setCopied(false)
    }
  }
  return (
    <section
      aria-label={t('bots:computer.addTitle')}
      className="mt-2 rounded-xl border border-stroke bg-surface-subtle p-3 text-sm"
    >
      <div className="flex items-center justify-between gap-2">
        <h4 className="font-semibold text-content">{t('bots:computer.addTitle')}</h4>
        <Button size="sm" variant="ghost" onClick={onDone}>
          {t('bots:computer.done')}
        </Button>
      </div>
      <p className="mt-1 text-xs text-content-muted">
        {t('bots:computer.code')}{' '}
        <b data-pairing-code className="font-mono text-content">
          {pairing.code}
        </b>{' '}
        · {t('bots:computer.codeHint')}
      </p>
      {!pairing.remote.enabled && (
        <div className="mt-2 flex flex-wrap items-center gap-2 rounded-lg border border-st-attn-border bg-surface-raised px-2.5 py-2 text-xs text-content-secondary">
          <span className="min-w-0 flex-1">{t('bots:computer.remoteOff')}</span>
          <Button size="sm" variant="outline" onClick={() => navigate('/config/remote-access')}>
            {t('bots:computer.openRemote')}
          </Button>
        </div>
      )}
      <p className="mt-2 text-xs text-content-muted">{t('bots:computer.runThere')}</p>
      <pre className="mt-1 max-h-40 overflow-auto rounded-lg bg-surface-raised p-2.5 font-mono text-xs whitespace-pre-wrap break-all text-content-secondary">
        {commands.join('\n')}
      </pre>
      <div className="mt-2 flex items-center justify-between gap-2">
        <p className="text-xs text-content-muted">{t('bots:computer.rootHint')}</p>
        <Button size="sm" variant="outline" onClick={() => void copy()}>
          <Copy aria-hidden="true" />
          {copied ? t('bots:computer.copied') : t('bots:computer.copy')}
        </Button>
      </div>
    </section>
  )
}

export function BotComputerTab({
  bot,
  workingTask,
  environments,
  onEnvironmentsChanged,
  onError,
}: {
  bot: Bot
  workingTask: BotTask | null
  environments: BotEnvironment[]
  onEnvironmentsChanged: () => void
  onError: (message: string) => void
}) {
  const { t } = useI18n()
  const upsert = useBotsStore((state) => state.upsert)
  const [environmentId, setEnvironmentId] = useState(bot.workspace.environmentId)
  const [cwd, setCwd] = useState(bot.workspace.cwd)
  const [picking, setPicking] = useState(false)
  const [saving, setSaving] = useState(false)
  const [pairing, setPairing] = useState<Pairing | null>(null)
  useEffect(() => {
    setEnvironmentId(bot.workspace.environmentId)
    setCwd(bot.workspace.cwd)
  }, [bot.id, bot.workspace.environmentId, bot.workspace.cwd])

  const selected = environments.find((environment) => environment.id === environmentId)
  const local = environmentId === 'local'
  const dirty = environmentId !== bot.workspace.environmentId || cwd.trim() !== bot.workspace.cwd
  const save = async (workspace: Partial<Bot['workspace']>) => {
    setSaving(true)
    try {
      upsert(await updateBot(bot.id, { workspace: { ...bot.workspace, ...workspace } }))
      onEnvironmentsChanged()
    } catch (caught) {
      onError(caught instanceof Error ? caught.message : String(caught))
    } finally {
      setSaving(false)
    }
  }
  const startPairing = async () => {
    try {
      setPairing(await createEnvironmentPairing())
    } catch (caught) {
      onError(caught instanceof Error ? caught.message : String(caught))
    }
  }

  return (
    <div className="flex flex-col gap-3">
      <p className="text-xs text-content-muted">{t('bots:computer.description')}</p>
      <label className="flex items-center justify-between gap-3 text-sm">
        <span className="flex-none">{t('bots:computer.runsOn')}</span>
        <AppSelect
          value={environmentId}
          className="w-48"
          aria-label={t('bots:computer.runsOn')}
          disabled={saving}
          onChange={(event) => {
            setEnvironmentId(event.target.value)
            // 换到别的电脑时目录要重新填：本机的路径在那边没有意义。
            setCwd(event.target.value === bot.workspace.environmentId ? bot.workspace.cwd : '')
          }}
        >
          {environments.length === 0 && <option value="local">{t('bots:computer.local')}</option>}
          {environments.map((environment) => (
            <option key={environment.id} value={environment.id}>
              {environment.kind === 'local'
                ? t('bots:computer.local')
                : t('bots:computer.option', {
                    name: environmentLabel(t, environment),
                    state: environment.online
                      ? t('bots:computer.online')
                      : t('bots:computer.offline'),
                  })}
            </option>
          ))}
        </AppSelect>
      </label>
      <div className="flex flex-col gap-1.5">
        <span className="text-xs font-medium text-content-muted">{t('bots:computer.folder')}</span>
        <div className="flex items-center gap-2">
          <Monitor className="size-3.5 flex-none text-content-muted" aria-hidden="true" />
          <Input
            value={cwd}
            className="font-mono text-xs"
            aria-label={t('bots:computer.folder')}
            placeholder={
              local
                ? t('bots:computer.folderPlaceholder')
                : (selected?.roots[0] ?? t('bots:computer.folderPlaceholder'))
            }
            disabled={saving}
            onChange={(event) => setCwd(event.target.value)}
          />
          {local && (
            <Button size="sm" variant="outline" disabled={saving} onClick={() => setPicking(true)}>
              {t('bots:new.pick')}
            </Button>
          )}
        </div>
        {!local && selected && (
          <p className="text-xs text-content-muted">
            {t('bots:computer.folderInRoots', { roots: selected.roots.join('、') })}
          </p>
        )}
        {dirty && (
          <div className="flex justify-end">
            <Button
              size="sm"
              disabled={saving || !cwd.trim()}
              onClick={() => void save({ environmentId, cwd: cwd.trim() })}
            >
              {t('bots:computer.switch')}
            </Button>
          </div>
        )}
      </div>
      {workingTask?.workspace?.branch && (
        <p className="text-xs text-content-muted">
          {t('bots:computer.currentBranch', { branch: workingTask.workspace.branch })}
        </p>
      )}
      <section aria-label={t('bots:computer.listTitle')} className="border-t border-stroke pt-3">
        <h4 className="text-xs font-medium text-content-muted">{t('bots:computer.listTitle')}</h4>
        <ul role="list" className="m-0 list-none divide-y divide-stroke p-0">
          {environments.map((environment) => (
            <EnvironmentRow
              key={environment.id}
              environment={environment}
              onRemoved={onEnvironmentsChanged}
              onError={onError}
            />
          ))}
        </ul>
        {pairing ? (
          <PairingPanel
            pairing={pairing}
            onDone={() => {
              setPairing(null)
              onEnvironmentsChanged()
            }}
          />
        ) : (
          <Button size="sm" variant="outline" className="mt-1" onClick={() => void startPairing()}>
            <Plus aria-hidden="true" />
            {t('bots:computer.add')}
          </Button>
        )}
      </section>
      <WorkspacePicker
        open={picking}
        initialPath={cwd || bot.workspace.cwd}
        onOpenChange={setPicking}
        onSelect={(path) => {
          setCwd(path)
          setPicking(false)
        }}
      />
    </div>
  )
}
