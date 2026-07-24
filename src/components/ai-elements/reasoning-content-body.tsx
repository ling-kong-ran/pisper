import { cjk } from '@streamdown/cjk'
import { code } from '@streamdown/code'
import { math } from '@streamdown/math'
import { Streamdown } from 'streamdown'

// Keep mermaid out of the default bundle; reasoning rarely needs diagrams and
// @streamdown/mermaid pulls a large graph of layout engines into the desktop app.
const streamdownPlugins = { cjk, code, math }

export function ReasoningContentBody({ children }: { children: string }) {
  return <Streamdown plugins={streamdownPlugins}>{children}</Streamdown>
}
