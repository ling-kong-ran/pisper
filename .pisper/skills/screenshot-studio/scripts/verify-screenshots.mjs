// Verify every captured PNG before replacing any file under docs/shots/.
import { copyFileSync, readFileSync, statSync } from 'node:fs'
import { resolve } from 'node:path'
import { RUN_DIR, SHOTS_DIR } from './screenshot-config.mjs'

// Web shots captured by capture-screenshots.mjs. TUI/demo assets stay untouched.
const WEB_SHOTS = [
  'assets',
  'channels',
  'chat-grid',
  'chat',
  'config',
  'config-desktop-pet',
  'config-interface',
  'config-notifications',
  'config-updates',
  'history',
  'mcp',
  'memory',
  'plugins',
  'schedules',
  'session-labels',
  'session-tree',
  'skills',
  'terminal',
  'turn-label',
  'welcome-dark',
  'workflow-builder',
  'workflows',
]
const EXPECTED_SIZE = '2558x1380'
const PNG_SIGNATURE = Buffer.from([137, 80, 78, 71, 13, 10, 26, 10])

function dimensions(file) {
  const data = readFileSync(file)
  if (data.length < 24 || !data.subarray(0, 8).equals(PNG_SIGNATURE)) {
    throw new Error('invalid PNG signature')
  }
  if (data.toString('ascii', 12, 16) !== 'IHDR') throw new Error('missing PNG IHDR')
  return `${data.readUInt32BE(16)}x${data.readUInt32BE(20)}`
}

const verified = []
let failed = false
for (const name of WEB_SHOTS) {
  const source = resolve(RUN_DIR, `${name}.png`)
  try {
    statSync(source)
    const size = dimensions(source)
    if (size !== EXPECTED_SIZE) {
      console.error(`BAD SIZE ${name}.png -> ${size} (expected ${EXPECTED_SIZE})`)
      failed = true
      continue
    }
    verified.push({ name, source, size })
  } catch (error) {
    console.error(`INVALID ${name}.png -> ${error instanceof Error ? error.message : String(error)}`)
    failed = true
  }
}

if (failed || verified.length !== WEB_SHOTS.length) {
  console.error('Screenshot verification failed; docs/shots was not modified.')
  process.exit(1)
}

for (const { name, source, size } of verified) {
  copyFileSync(source, resolve(SHOTS_DIR, `${name}.png`))
  console.log(`replaced docs/shots/${name}.png (${size})`)
}
console.log(`All ${verified.length} Web screenshots verified and replaced.`)
