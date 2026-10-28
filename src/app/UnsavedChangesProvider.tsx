import {
  useCallback,
  useLayoutEffect,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from 'react'
import { useBlocker, useLocation, useNavigate } from 'react-router-dom'
import { useI18n } from '@/app/use-i18n'
import { Button } from '@/components/ui/button'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import {
  GUARDED_NAVIGATION_STATE,
  navigationChangesContent,
  UnsavedChangesContext,
  unsavedSummary,
  type GuardedNavigate,
  type UnsavedChangesSource,
} from './contracts/unsaved-changes'

type Pending = { kind: 'route' } | { kind: 'close'; id: string; close: () => void }
export function UnsavedChangesProvider({ children }: { children: ReactNode }) {
  const { t } = useI18n()
  const routerNavigate = useNavigate()
  const location = useLocation()
  const sources = useRef(new Map<string, UnsavedChangesSource>())
  const callbacks = useRef(new Map<string, () => void>())
  const sequence = useRef(0)
  const [revision, setRevision] = useState(0)
  const [pending, setPending] = useState<Pending | null>(null)
  const pendingRef = useRef<Pending | null>(null)
  const [savingNotice, setSavingNotice] = useState(false)
  const blocker = useBlocker(
    ({ currentLocation, nextLocation }) =>
      navigationChangesContent(currentLocation, nextLocation) &&
      Object.values(unsavedSummary(sources.current.values())).some(Boolean),
  )
  const blockerRef = useRef(blocker)
  blockerRef.current = blocker
  const setIntent = useCallback((next: Pending | null) => {
    pendingRef.current = next
    setPending(next)
  }, [])
  const update = useCallback((source: UnsavedChangesSource) => {
    const before = sources.current.get(source.id)
    sources.current.set(source.id, source)
    if (!before || before.dirty !== source.dirty || before.saving !== source.saving)
      setRevision((value) => value + 1)
  }, [])
  const remove = useCallback((id: string) => {
    if (sources.current.delete(id)) setRevision((value) => value + 1)
  }, [])
  const cancel = useCallback(() => {
    setIntent(null)
    callbacks.current.clear()
    if (blockerRef.current.state === 'blocked') blockerRef.current.reset()
  }, [setIntent])
  const navigate = useCallback<GuardedNavigate>(
    (to, options, onCommit) => {
      callbacks.current.clear()
      const id = `navigation-${++sequence.current}`
      if (onCommit) callbacks.current.set(id, onCommit)
      void routerNavigate(to, {
        ...options,
        state: { ...options?.state, [GUARDED_NAVIGATION_STATE]: id },
      })
    },
    [routerNavigate],
  )
  const requestClose = useCallback(
    (id: string, close: () => void) => {
      const source = sources.current.get(id)
      if (source?.saving) {
        cancel()
        setSavingNotice(true)
        return
      }
      if (blockerRef.current.state === 'blocked') blockerRef.current.reset()
      callbacks.current.clear()
      if (!source?.dirty) {
        setIntent(null)
        close()
        return
      }
      setSavingNotice(false)
      setIntent({ kind: 'close', id, close })
    },
    [cancel, setIntent],
  )
  useLayoutEffect(() => {
    const summary = unsavedSummary(sources.current.values())
    if (blocker.state === 'blocked') {
      if (summary.saving || !summary.dirty) {
        cancel()
        setSavingNotice(summary.saving)
      } else {
        setSavingNotice(false)
        setIntent({ kind: 'route' })
      }
    } else if (pendingRef.current?.kind === 'close') {
      const source = sources.current.get(pendingRef.current.id)
      if (!source || source.saving || !source.dirty) {
        setIntent(null)
        setSavingNotice(Boolean(source?.saving))
      }
    }
  }, [blocker, revision, cancel, setIntent])
  useLayoutEffect(() => {
    const id = location.state?.[GUARDED_NAVIGATION_STATE]
    const action = typeof id === 'string' ? callbacks.current.get(id) : undefined
    if (action) {
      callbacks.current.delete(id)
      action()
    }
  }, [location])
  const summary = unsavedSummary(sources.current.values())
  useEffect(() => {
    if (!summary.dirty && !summary.saving) return
    const prevent = (event: BeforeUnloadEvent) => {
      event.preventDefault()
      event.returnValue = ''
    }
    window.addEventListener('beforeunload', prevent)
    return () => window.removeEventListener('beforeunload', prevent)
  }, [summary.dirty, summary.saving])
  const discard = () => {
    const intent = pendingRef.current
    if (!intent) return
    const affected =
      intent.kind === 'close'
        ? [sources.current.get(intent.id)].filter((source): source is UnsavedChangesSource =>
            Boolean(source),
          )
        : [...sources.current.values()]
    if (affected.some((source) => source.saving)) {
      cancel()
      setSavingNotice(true)
      return
    }
    // 先取出并清除动作，双击或重复 Enter 无法再次执行。
    setIntent(null)
    for (const source of affected) if (source.dirty) source.discard()
    if (intent.kind === 'close') intent.close()
    else if (blockerRef.current.state === 'blocked') blockerRef.current.proceed()
  }
  const value = useMemo(
    () => ({ update, remove, requestClose, navigate }),
    [update, remove, requestClose, navigate],
  )
  return (
    <UnsavedChangesContext.Provider value={value}>
      {children}
      {savingNotice && (
        <p
          role="status"
          className="fixed right-4 bottom-4 z-[300] rounded-lg border bg-popover p-3 text-sm text-popover-foreground shadow-md"
        >
          {t('common:unsaved.saving')}
        </p>
      )}
      <Dialog
        open={Boolean(pending)}
        onOpenChange={(open) => {
          if (!open) cancel()
        }}
      >
        <DialogContent
          size="compact"
          surface="confirmation"
          showCloseButton={false}
          className="z-[400]"
          overlayClassName="z-[400]"
        >
          <DialogHeader>
            <DialogTitle>{t('common:unsaved.title')}</DialogTitle>
            <DialogDescription>{t('common:unsaved.description')}</DialogDescription>
          </DialogHeader>
          <form
            onSubmit={(event) => {
              event.preventDefault()
              discard()
            }}
          >
            <DialogFooter>
              <Button
                type="button"
                variant="outline"
                size="dialogAction"
                onClick={cancel}
                autoFocus
              >
                {t('common:unsaved.keepEditing')}
              </Button>
              <Button type="submit" variant="destructive" size="dialogAction">
                {t('common:unsaved.discard')}
              </Button>
            </DialogFooter>
          </form>
        </DialogContent>
      </Dialog>
    </UnsavedChangesContext.Provider>
  )
}
