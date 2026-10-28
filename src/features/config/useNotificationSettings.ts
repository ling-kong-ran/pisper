import { useCallback, useEffect, useRef, useState } from 'react'
import { invalidResponseError } from '@/lib/http-response'
import { notificationApi, supportsNotificationChannelPreferences } from './notification-api'
import type { NotificationPlatform, NotificationSettingsData } from '@/types/notifications'

type BooleanTarget =
  | { kind: 'browser' }
  | { kind: 'event'; event: string }
  | { kind: 'channel'; event: string; platform: NotificationPlatform }
type Mutation = { target: BooleanTarget; value: boolean; pending: boolean; error: string }
type Intent = { target: BooleanTarget; value: boolean; revision: number }
type Draft = { event: string; platform: NotificationPlatform; content: string; revision: number }
type RequestState = { pending: boolean; error: string }
const message = (error: unknown) => (error instanceof Error ? error.message : String(error))
const draftKey = (event: string, platform: NotificationPlatform) => `${event}\0${platform}`
const targetKey = (target: BooleanTarget) =>
  target.kind === 'browser'
    ? 'browser'
    : target.kind === 'event'
      ? `event:${target.event}`
      : `channel:${target.event}:${target.platform}`
function booleanValue(data: NotificationSettingsData, target: BooleanTarget) {
  if (target.kind === 'browser') return data.browser.enabled
  const template = data.templates.find((item) => item.id === target.event)
  return target.kind === 'event'
    ? template?.enabled === true
    : template?.channels[target.platform]?.enabled !== false
}
function withBoolean(
  data: NotificationSettingsData,
  target: BooleanTarget,
  value: boolean,
): NotificationSettingsData {
  if (target.kind === 'browser') return { ...data, browser: { enabled: value } }
  return {
    ...data,
    templates: data.templates.map((template) =>
      template.id !== target.event
        ? template
        : target.kind === 'event'
          ? { ...template, enabled: value }
          : {
              ...template,
              channels: {
                ...template.channels,
                [target.platform]: { ...template.channels[target.platform]!, enabled: value },
              },
            },
    ),
  }
}

export function useNotificationSettings(onConfirmed?: (data: NotificationSettingsData) => void) {
  const [confirmed, setConfirmed] = useState<NotificationSettingsData | null>(null)
  const snapshot = useRef<NotificationSettingsData | null>(null)
  const projection = useRef(onConfirmed)
  projection.current = onConfirmed
  const [loading, setLoading] = useState(true)
  const [loadError, setLoadError] = useState('')
  const [mutations, setMutations] = useState<Record<string, Mutation>>({})
  const [drafts, setDrafts] = useState<Record<string, Draft>>({})
  const draftRef = useRef<Record<string, Draft>>({})
  const [saves, setSaves] = useState<Record<string, RequestState>>({})
  const [tests, setTests] = useState<Record<string, RequestState>>({})
  const alive = useRef(false)
  const read = useRef<AbortController | null>(null)
  const writers = useRef(new Map<string, AbortController>())
  const intents = useRef(new Map<string, Intent>())
  const uncertain = useRef(new Set<string>())
  const revision = useRef(0)
  const snapshotVersion = useRef(0)
  const publish = useCallback((next: NotificationSettingsData) => {
    snapshot.current = next
    snapshotVersion.current++
    setConfirmed(next)
    projection.current?.(next)
  }, [])
  const load = useCallback(async () => {
    if (!alive.current) return
    read.current?.abort()
    const controller = new AbortController()
    read.current = controller
    const version = snapshotVersion.current
    setLoading(true)
    setLoadError('')
    try {
      const result = await notificationApi.load({ signal: controller.signal })
      if (alive.current && !controller.signal.aborted && version === snapshotVersion.current)
        publish(result)
    } catch (error) {
      if (alive.current && !controller.signal.aborted) setLoadError(message(error))
    } finally {
      if (alive.current && !controller.signal.aborted) setLoading(false)
    }
  }, [publish])
  useEffect(() => {
    alive.current = true
    void load()
    const requests = writers.current
    const queued = intents.current
    return () => {
      alive.current = false
      read.current?.abort()
      for (const controller of requests.values()) controller.abort()
      requests.clear()
      queued.clear()
    }
  }, [load])

  const mutate = async (target: BooleanTarget, value: boolean) => {
    if (!alive.current || !snapshot.current) return
    const key = targetKey(target)
    const intent = { target, value, revision: ++revision.current }
    intents.current.set(key, intent)
    setMutations((current) => ({ ...current, [key]: { target, value, pending: true, error: '' } }))
    if (writers.current.has(key)) return
    const controller = new AbortController()
    writers.current.set(key, controller)
    try {
      while (intents.current.has(key) && alive.current && !controller.signal.aborted) {
        const submitted = intents.current.get(key)!
        intents.current.delete(key)
        try {
          if (
            booleanValue(snapshot.current!, target) !== submitted.value ||
            uncertain.current.has(key)
          ) {
            const options = { signal: controller.signal }
            const result =
              target.kind === 'browser'
                ? await notificationApi.setBrowserEnabled(submitted.value, options)
                : target.kind === 'event'
                  ? await notificationApi.setEventEnabled(target.event, submitted.value, options)
                  : await notificationApi.setChannelEnabled(
                      target.event,
                      target.platform,
                      submitted.value,
                      options,
                    )
            if (!alive.current || controller.signal.aborted) return
            publish(withBoolean(snapshot.current!, target, booleanValue(result, target)))
            uncertain.current.delete(key)
          }
          if (!intents.current.has(key))
            setMutations((current) => ({
              ...current,
              [key]: { target, value: submitted.value, pending: false, error: '' },
            }))
        } catch (error) {
          if (!alive.current || controller.signal.aborted) return
          uncertain.current.add(key)
          // 同键较新意图优先；旧失败只能在没有后继意图时回滚到已确认快照。
          if (!intents.current.has(key))
            setMutations((current) => ({
              ...current,
              [key]: { target, value: submitted.value, pending: false, error: message(error) },
            }))
        }
      }
    } finally {
      if (writers.current.get(key) === controller) writers.current.delete(key)
    }
  }
  let data = confirmed
  if (data)
    for (const mutation of Object.values(mutations))
      if (mutation.pending) data = withBoolean(data, mutation.target, mutation.value)
  const editDraft = (event: string, platform: NotificationPlatform, content: string) => {
    const key = draftKey(event, platform)
    const next = {
      ...draftRef.current,
      [key]: { event, platform, content, revision: ++revision.current },
    }
    draftRef.current = next
    setDrafts(next)
  }
  const draft = (event: string, platform: NotificationPlatform) => {
    const key = draftKey(event, platform)
    const saved =
      confirmed?.templates.find((item) => item.id === event)?.channels[platform]?.content || ''
    const content = drafts[key]?.content ?? saved
    return {
      content,
      dirty: content !== saved,
      save: saves[key] || { pending: false, error: '' },
      test: tests[key] || { pending: false, error: '' },
    }
  }
  const saveTemplate = async (event: string, platform: NotificationPlatform) => {
    const key = draftKey(event, platform)
    const requestKey = `content:${key}`
    const submitted = draftRef.current[key]
    if (
      !alive.current ||
      !submitted ||
      !submitted.content.trim() ||
      writers.current.has(requestKey)
    )
      return false
    const controller = new AbortController()
    writers.current.set(requestKey, controller)
    setSaves((current) => ({ ...current, [key]: { pending: true, error: '' } }))
    try {
      const result = await notificationApi.saveTemplate(event, platform, submitted.content, {
        signal: controller.signal,
      })
      if (!alive.current || controller.signal.aborted) return false
      const content = result.templates.find((item) => item.id === event)?.channels[platform]
        ?.content
      if (content === undefined) throw invalidResponseError()
      publish({
        ...snapshot.current!,
        templates: snapshot.current!.templates.map((template) =>
          template.id !== event
            ? template
            : {
                ...template,
                channels: {
                  ...template.channels,
                  [platform]: { ...template.channels[platform]!, content },
                },
              },
        ),
      })
      const savedCurrent = draftRef.current[key]?.revision === submitted.revision
      if (savedCurrent) {
        draftRef.current = { ...draftRef.current, [key]: { ...submitted, content } }
        setDrafts(draftRef.current)
      }
      setSaves((current) => ({ ...current, [key]: { pending: false, error: '' } }))
      return savedCurrent
    } catch (error) {
      if (alive.current && !controller.signal.aborted)
        setSaves((current) => ({ ...current, [key]: { pending: false, error: message(error) } }))
      return false
    } finally {
      if (writers.current.get(requestKey) === controller) writers.current.delete(requestKey)
    }
  }
  const testTemplate = async (event: string, platform: NotificationPlatform) => {
    const key = draftKey(event, platform)
    const requestKey = `test:${key}`
    if (!alive.current || writers.current.has(requestKey) || draft(event, platform).dirty) return
    const controller = new AbortController()
    writers.current.set(requestKey, controller)
    setTests((current) => ({ ...current, [key]: { pending: true, error: '' } }))
    try {
      const result = await notificationApi.testSavedTemplate(event, platform, {
        signal: controller.signal,
      })
      if (!alive.current || controller.signal.aborted) return
      setTests((current) => ({ ...current, [key]: { pending: false, error: '' } }))
      return result
    } catch (error) {
      if (alive.current && !controller.signal.aborted)
        setTests((current) => ({ ...current, [key]: { pending: false, error: message(error) } }))
    } finally {
      if (writers.current.get(requestKey) === controller) writers.current.delete(requestKey)
    }
  }
  const dirty = Object.values(drafts).some(
    (item) =>
      item.content !==
      (confirmed?.templates.find((template) => template.id === item.event)?.channels[item.platform]
        ?.content || ''),
  )
  const saving =
    Object.values(mutations).some((item) => item.pending) ||
    Object.values(saves).some((item) => item.pending)
  const discard = () => {
    draftRef.current = {}
    setDrafts({})
  }
  return {
    dirty,
    saving,
    discard,
    data,
    confirmed,
    loading,
    loadError,
    load,
    mutations,
    supportsChannels: confirmed ? supportsNotificationChannelPreferences(confirmed) : false,
    setBrowserEnabled: (value: boolean) => mutate({ kind: 'browser' }, value),
    setEventEnabled: (event: string, value: boolean) => mutate({ kind: 'event', event }, value),
    setChannelEnabled: (event: string, platform: NotificationPlatform, value: boolean) =>
      mutate({ kind: 'channel', event, platform }, value),
    retryMutation: (mutation: Mutation) => mutate(mutation.target, mutation.value),
    draft,
    editDraft,
    saveTemplate,
    testTemplate,
  }
}
