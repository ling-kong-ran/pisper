// bot_schedule 与 bot_ask 的纯逻辑：把“给自己定时”的输入换成定时任务的字段（只能发给 Bot 自己），
// 把问题卡片的回答写成说回对话的一句话。
import { pisperError } from '../../../shared/errors.mjs'

const DEFAULT_TIMEZONE = 'Asia/Hong_Kong'

function localTimezone() {
  try {
    return Intl.DateTimeFormat().resolvedOptions().timeZone || DEFAULT_TIMEZONE
  } catch {
    return DEFAULT_TIMEZONE
  }
}

// 某一刻在某时区的日期与时刻（YYYY-MM-DD、HH:MM）。
export function zonedDateTime(at, timeZone) {
  const parts = Object.fromEntries(
    new Intl.DateTimeFormat('en-CA', {
      timeZone,
      year: 'numeric',
      month: '2-digit',
      day: '2-digit',
      hour: '2-digit',
      minute: '2-digit',
      hourCycle: 'h23',
    })
      .formatToParts(new Date(at))
      .map((part) => [part.type, part.value]),
  )
  return {
    date: `${parts.year}-${parts.month}-${parts.day}`,
    time: `${parts.hour}:${parts.minute}`,
  }
}

/**
 * @param {{ bot: { id: string, name: string }, input: Record<string, any>, now?: number, timezone?: string }} options
 */
export function botScheduleInput({ bot, input, now = Date.now(), timezone = localTimezone() }) {
  const message = String(input?.message || '').trim()
  if (!message)
    throw pisperError('invalid', 'schedule', 'message: say what to tell you when the time comes')
  const base = {
    targetType: 'bot',
    botId: bot.id,
    botMessage: message.slice(0, 4_000),
    name: `${bot.name}: ${message.slice(0, 40)}`,
    enabled: true,
    timezone,
  }
  if (input?.repeat === 'daily' || input?.repeat === 'weekly') {
    if (!/^([01]\d|2[0-3]):[0-5]\d$/.test(String(input.time || '')))
      throw pisperError('invalid', 'schedule', 'a repeating schedule needs time (HH:MM)')
    return {
      ...base,
      frequency: input.repeat,
      time: input.time,
      ...(input.repeat === 'weekly' ? { dayOfWeek: Number(input.weekday ?? 1) } : {}),
    }
  }
  // 一次性：多少分钟后（向上取整到下一分钟），或者某天某时。
  if (Number.isInteger(input?.inMinutes) && input.inMinutes > 0) {
    const at = Math.ceil((now + input.inMinutes * 60_000) / 60_000) * 60_000
    return { ...base, frequency: 'once', ...zonedDateTime(at, timezone) }
  }
  if (/^\d{4}-\d{2}-\d{2}$/.test(String(input?.date || '')) && input?.time)
    return { ...base, frequency: 'once', date: input.date, time: input.time }
  throw pisperError('invalid', 'schedule', 'say when: inMinutes, date + time, or repeat + time')
}

// 问题卡片的回答说回对话的那句话在 runtime/prompts/bots.mjs，这里按原名转出。
export { botAnswerText } from '../../prompts/bots.mjs'
