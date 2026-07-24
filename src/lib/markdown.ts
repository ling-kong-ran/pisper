import remend from 'remend'

export function prepareMarkdown(value: unknown, streaming = false): string {
  const source = String(value || '')
  if (!streaming) return source
  return remend(source, { linkMode: 'text-only' })
}
