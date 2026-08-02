import { cjk } from '@streamdown/cjk'
import { createCodePlugin } from '@streamdown/code'
import { math } from '@streamdown/math'
import type { PluginConfig } from 'streamdown'

const streamdownCode = createCodePlugin({ themes: ['github-dark', 'github-dark'] })

export const streamdownPlugins = { cjk, code: streamdownCode, math } satisfies PluginConfig
