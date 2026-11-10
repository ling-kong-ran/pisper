// 远程访问页里的“远程机器”：让 Agent 在别的机器上工作，对方不用装 Pisper。两种连法在这里并排：
// 有 SSH 服务的走 SSH（用这台电脑现有的配置和密钥，或用户名加密码）；没有的让对方运行一条命令连过来。
import './machines-messages'
import { useId, useState, type FormEvent } from 'react'
import { useNavigate } from 'react-router-dom'
import { Copy, Laptop, Pencil, Plus, RefreshCw, Server, Trash2 } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { FeatureAlert, FeatureList, FeatureRow, FeatureSection } from '@/components/app/FeaturePage'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { SegmentedControl } from '@/components/ui/segmented-control'
import { OrbitSpinner } from '@/components/ui/spinner'
import type { ConfirmDialogOptions } from '@/hooks/useAppDialog'
import { cn } from '@/lib/utils'
import { executorCommands, machineFailureMessage } from './machine-messages'
import {
  checkMachine,
  createMachinePairing,
  createSshMachine,
  machineErrorCode,
  removeMachine,
  testSshConnection,
  updateSshMachine,
  type Machine,
  type MachineAuth,
  type MachinePairing,
  type SshTestResult,
} from './machines-api'
import { useMachines } from './useMachines'

function machineAddress(machine: Machine) {
  const system = machine.system?.name ? ` · ${machine.system.name}` : ''
  if (machine.kind === 'node') return machine.roots.join(' · ') + system
  return (machine.port ? `${machine.target}:${machine.port}` : machine.target) + system
}

// 添加和编辑共用一张表单。登录方式二选一：密钥（推荐，什么都不存）或密码（存在这台电脑上）。
// 编辑密码机器时密码框留空 = 保留已存的密码；改回密钥会删掉它。密码只发给 Runtime，不会再读回来。
function SshForm({
  machine,
  onDone,
  onCancel,
}: {
  machine?: Machine
  onDone: () => void
  onCancel: () => void
}) {
  const { t } = useI18n()
  const fieldId = useId()
  const [name, setName] = useState(machine?.name ?? '')
  const [target, setTarget] = useState(machine?.target ?? '')
  const [port, setPort] = useState(machine?.port ? String(machine.port) : '')
  const [defaultCwd, setDefaultCwd] = useState(machine?.defaultCwd ?? '')
  const [auth, setAuth] = useState<MachineAuth>(machine?.auth ?? 'key')
  const [password, setPassword] = useState('')
  const [busy, setBusy] = useState<'' | 'test' | 'save'>('')
  const [result, setResult] = useState<SshTestResult | null>(null)
  const [error, setError] = useState('')
  // 存着的密码只属于存它时的地址和端口：改了其中一个就要重新输入，Runtime 也不会把旧密码发到新地址。
  const addressChanged =
    Boolean(machine) &&
    (target.trim() !== machine?.target ||
      (port.trim() ? Number(port.trim()) : null) !== (machine?.port ?? null))
  const hadPassword = Boolean(machine?.passwordSet) && machine?.auth === 'password'
  const savedPassword = hadPassword && !addressChanged
  const needsPassword = auth === 'password' && !password && !savedPassword
  const connection = () => ({
    target: target.trim(),
    port: port.trim() ? Number(port.trim()) : null,
    auth,
    ...(auth === 'password' && password ? { password } : {}),
  })
  const failed = (caught: unknown) =>
    setError(
      machineFailureMessage(
        t,
        machineErrorCode(caught),
        caught instanceof Error ? caught.message : '',
        { auth },
      ),
    )
  const test = async () => {
    setBusy('test')
    setError('')
    setResult(null)
    try {
      setResult(
        await testSshConnection({ ...connection(), ...(machine ? { id: machine.id } : {}) }),
      )
    } catch (caught) {
      failed(caught)
    } finally {
      setBusy('')
    }
  }
  const save = async (event: FormEvent) => {
    event.preventDefault()
    if (needsPassword) return
    setBusy('save')
    setError('')
    try {
      const input = { name: name.trim(), ...connection(), defaultCwd: defaultCwd.trim() }
      const saved = machine
        ? await updateSshMachine(machine.id, input)
        : await createSshMachine(input)
      // 保存后顺手连一次，列表里马上能看到通不通；连不上也已经存下了。
      await checkMachine(saved.id).catch(() => undefined)
      setPassword('')
      onDone()
    } catch (caught) {
      failed(caught)
    } finally {
      setBusy('')
    }
  }
  return (
    <form
      onSubmit={(event) => void save(event)}
      data-machine-ssh-form={machine ? 'edit' : 'add'}
      aria-label={machine ? t('machines:edit', { name: machine.name }) : t('machines:addSsh')}
      className="mt-3 grid gap-3 rounded-[var(--r-md)] border border-stroke bg-surface-subtle p-3"
    >
      <div className="grid gap-3 sm:grid-cols-[minmax(0,1fr)_7rem]">
        <label className="grid gap-1.5 text-xs font-medium text-content-secondary">
          {t('machines:form.target')}
          <Input
            id={`${fieldId}-target`}
            required
            autoFocus
            spellCheck={false}
            value={target}
            className="font-mono text-xs"
            placeholder={t('machines:form.targetPlaceholder')}
            data-machine-target
            onChange={(event) => setTarget(event.target.value)}
          />
        </label>
        <label className="grid gap-1.5 text-xs font-medium text-content-secondary">
          {t('machines:form.port')}
          <Input
            inputMode="numeric"
            value={port}
            className="font-mono text-xs"
            placeholder="22"
            data-machine-port
            onChange={(event) => setPort(event.target.value)}
          />
        </label>
      </div>
      <div className="grid gap-1.5 text-xs font-medium text-content-secondary">
        <span id={`${fieldId}-auth`}>{t('machines:form.auth')}</span>
        <SegmentedControl<MachineAuth>
          aria-labelledby={`${fieldId}-auth`}
          value={auth}
          onValueChange={(next) => {
            setAuth(next)
            setResult(null)
          }}
          options={[
            { value: 'key', label: t('machines:form.authKey') },
            { value: 'password', label: t('machines:form.authPassword') },
          ]}
          data-machine-auth
        />
      </div>
      {auth === 'password' && (
        <label className="grid gap-1.5 text-xs font-medium text-content-secondary">
          {t('machines:form.password')}
          <Input
            type="password"
            autoComplete="new-password"
            spellCheck={false}
            value={password}
            required={!savedPassword}
            placeholder={savedPassword ? t('machines:form.passwordKeep') : ''}
            aria-describedby={`${fieldId}-password-hint`}
            data-machine-password
            onChange={(event) => setPassword(event.target.value)}
          />
        </label>
      )}
      <div className="grid gap-3 sm:grid-cols-2">
        <label className="grid gap-1.5 text-xs font-medium text-content-secondary">
          {t('machines:form.name')}
          <Input
            value={name}
            placeholder={t('machines:form.namePlaceholder')}
            data-machine-name
            onChange={(event) => setName(event.target.value)}
          />
        </label>
        <label className="grid gap-1.5 text-xs font-medium text-content-secondary">
          {t('machines:form.defaultCwd')}
          <Input
            spellCheck={false}
            value={defaultCwd}
            className="font-mono text-xs"
            placeholder="~"
            onChange={(event) => setDefaultCwd(event.target.value)}
          />
        </label>
      </div>
      <p id={`${fieldId}-password-hint`} className="m-0 text-xs text-content-muted">
        {auth === 'password' && hadPassword && addressChanged && (
          <span data-machine-password-again>{t('machines:form.passwordAgain')} </span>
        )}
        {auth === 'password' ? t('machines:form.passwordHint') : t('machines:form.keyHint')}
      </p>
      {result?.ok && (
        <p role="status" className="m-0 text-xs text-success" data-machine-test-ok>
          {t('machines:form.testOk', {
            platform: result.platform,
            home: result.home,
          })}
        </p>
      )}
      {result && !result.ok && (
        <FeatureAlert className="mt-0" data-machine-test-failed>
          {machineFailureMessage(t, result.code, result.message, { auth })}
        </FeatureAlert>
      )}
      {error && <FeatureAlert className="mt-0">{error}</FeatureAlert>}
      <div className="flex flex-wrap justify-end gap-2">
        <Button type="button" variant="ghost" onClick={onCancel}>
          {t('machines:form.cancel')}
        </Button>
        <Button
          type="button"
          variant="outline"
          disabled={Boolean(busy) || !target.trim() || needsPassword}
          data-machine-test
          onClick={() => void test()}
        >
          {busy === 'test' && <OrbitSpinner className="size-3.5" />}
          {busy === 'test' ? t('machines:form.testing') : t('machines:form.test')}
        </Button>
        <Button
          type="submit"
          disabled={Boolean(busy) || !target.trim() || needsPassword}
          data-machine-save
        >
          {t('machines:form.save')}
        </Button>
      </div>
    </form>
  )
}

function PairingPanel({ pairing, onDone }: { pairing: MachinePairing; onDone: () => void }) {
  const { t } = useI18n()
  const [copied, setCopied] = useState(false)
  const commands = executorCommands(pairing, {
    brain: t('machines:pair.brainPlaceholder'),
    fingerprint: 'SHA256:…',
    root: t('machines:pair.rootPlaceholder'),
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
      aria-label={t('machines:addNode')}
      data-machine-pairing
      className="mt-3 grid gap-2 rounded-[var(--r-md)] border border-stroke bg-surface-subtle p-3 text-xs"
    >
      <p className="m-0 text-content-secondary">{t('machines:pair.description')}</p>
      {!pairing.remote.enabled && (
        <FeatureAlert className="mt-0" data-machine-remote-off>
          {t('machines:pair.remoteOff')}
        </FeatureAlert>
      )}
      <p className="m-0 text-content-muted">
        {t('machines:pair.code')}{' '}
        <b className="font-mono text-content" data-machine-pairing-code>
          {pairing.code}
        </b>{' '}
        · {t('machines:pair.codeHint')}
      </p>
      <p className="m-0 text-content-muted">{t('machines:pair.runThere')}</p>
      <pre className="m-0 max-h-40 overflow-auto rounded-[var(--r-sm)] bg-surface-raised p-2.5 font-mono break-all whitespace-pre-wrap text-content-secondary">
        {commands.join('\n')}
      </pre>
      <p className="m-0 text-content-muted">{t('machines:pair.rootHint')}</p>
      <div className="flex justify-end gap-2">
        <Button type="button" variant="outline" onClick={() => void copy()}>
          <Copy aria-hidden="true" />
          {copied ? t('machines:pair.copied') : t('machines:pair.copy')}
        </Button>
        <Button type="button" onClick={onDone}>
          {t('machines:pair.done')}
        </Button>
      </div>
    </section>
  )
}

function MachineStatus({ machine, checking }: { machine: Machine; checking: boolean }) {
  const { t } = useI18n()
  const unchecked = machine.kind === 'ssh' && !machine.lastCheck
  const label = checking
    ? t('machines:status.checking')
    : machine.kind === 'node'
      ? machine.online
        ? t('machines:status.online')
        : t('machines:status.offline')
      : unchecked
        ? t('machines:status.unchecked')
        : machine.online
          ? t('machines:status.reachable')
          : t('machines:status.unreachable')
  return (
    <span className="inline-flex items-center gap-1.5 text-xs text-content-muted">
      <span
        aria-hidden="true"
        className={cn(
          'size-2 flex-none rounded-full',
          machine.online ? 'bg-success' : unchecked ? 'bg-content-faint' : 'bg-st-attn',
        )}
      />
      {label}
    </span>
  )
}

export function RemoteMachinesSettings({
  requestConfirm,
}: {
  requestConfirm: (options?: ConfirmDialogOptions) => Promise<boolean>
}) {
  const { t } = useI18n()
  const navigate = useNavigate()
  const { available, machines, refresh } = useMachines()
  const [adding, setAdding] = useState(false)
  const [editing, setEditing] = useState<Machine | null>(null)
  const [pairing, setPairing] = useState<MachinePairing | null>(null)
  const [checking, setChecking] = useState('')
  const [failure, setFailure] = useState('')
  if (!available) return null

  const startPairing = async () => {
    setFailure('')
    setAdding(false)
    setEditing(null)
    try {
      setPairing(await createMachinePairing())
    } catch (caught) {
      setFailure(caught instanceof Error ? caught.message : String(caught))
    }
  }
  const recheck = async (machine: Machine) => {
    setChecking(machine.id)
    setFailure('')
    try {
      const result = await checkMachine(machine.id)
      if (!result.ok)
        setFailure(machineFailureMessage(t, result.code, result.message, { auth: machine.auth }))
      await refresh()
    } catch (caught) {
      setFailure(caught instanceof Error ? caught.message : String(caught))
    } finally {
      setChecking('')
    }
  }
  const remove = async (machine: Machine) => {
    const confirmed = await requestConfirm({
      title: t('machines:remove', { name: machine.name }),
      message: t('machines:removeDescription'),
    })
    if (!confirmed) return
    setFailure('')
    try {
      await removeMachine(machine.id)
      if (editing?.id === machine.id) setEditing(null)
      await refresh()
    } catch (caught) {
      setFailure(caught instanceof Error ? caught.message : String(caught))
    }
  }

  return (
    <FeatureSection
      title={t('machines:title')}
      count={machines.length}
      description={t('machines:description')}
      data-config-card="remote-machines"
      actions={
        <>
          <Button
            type="button"
            variant="outline"
            data-machine-add-ssh
            onClick={() => {
              setPairing(null)
              setEditing(null)
              setAdding(true)
            }}
          >
            <Plus aria-hidden="true" />
            {t('machines:addSsh')}
          </Button>
          <Button
            type="button"
            variant="outline"
            data-machine-add-node
            onClick={() => void startPairing()}
          >
            <Laptop aria-hidden="true" />
            {t('machines:addNode')}
          </Button>
        </>
      }
    >
      {machines.length > 0 && (
        <FeatureList>
          {machines.map((machine) => (
            <FeatureRow
              key={machine.id}
              data-machine={machine.id}
              icon={machine.kind === 'ssh' ? <Server /> : <Laptop />}
              name={machine.name || t('machines:unnamed')}
              description={<span className="font-mono">{machineAddress(machine)}</span>}
              status={<MachineStatus machine={machine} checking={checking === machine.id} />}
              actions={
                <>
                  {machine.kind === 'ssh' && (
                    <Button
                      type="button"
                      variant="ghost"
                      size="icon-sm"
                      aria-label={t('machines:edit', { name: machine.name })}
                      title={t('machines:edit', { name: machine.name })}
                      data-machine-edit
                      onClick={() => {
                        setAdding(false)
                        setPairing(null)
                        setEditing(machine)
                      }}
                    >
                      <Pencil className="size-3.5" />
                    </Button>
                  )}
                  {machine.kind === 'ssh' && (
                    <Button
                      type="button"
                      variant="ghost"
                      size="icon-sm"
                      disabled={Boolean(checking)}
                      aria-label={t('machines:recheck', { name: machine.name })}
                      title={t('machines:recheck', { name: machine.name })}
                      onClick={() => void recheck(machine)}
                    >
                      <RefreshCw className="size-3.5" />
                    </Button>
                  )}
                  <Button
                    type="button"
                    variant="ghost"
                    size="icon-sm"
                    className="hover:bg-danger-soft hover:text-danger"
                    aria-label={t('machines:remove', { name: machine.name })}
                    title={t('machines:remove', { name: machine.name })}
                    onClick={() => void remove(machine)}
                  >
                    <Trash2 className="size-3.5" />
                  </Button>
                </>
              }
            />
          ))}
        </FeatureList>
      )}
      {failure && <FeatureAlert data-machine-failure>{failure}</FeatureAlert>}
      {adding && (
        <SshForm
          onCancel={() => setAdding(false)}
          onDone={() => {
            setAdding(false)
            void refresh()
          }}
        />
      )}
      {editing && (
        <SshForm
          key={editing.id}
          machine={editing}
          onCancel={() => setEditing(null)}
          onDone={() => {
            setEditing(null)
            void refresh()
          }}
        />
      )}
      {pairing && (
        <>
          <PairingPanel
            pairing={pairing}
            onDone={() => {
              setPairing(null)
              void refresh()
            }}
          />
          {!pairing.remote.enabled && (
            <Button
              type="button"
              variant="ghost"
              className="mt-1"
              onClick={() => navigate('/config/remote-access')}
            >
              {t('machines:pair.openRemote')}
            </Button>
          )}
        </>
      )}
    </FeatureSection>
  )
}
