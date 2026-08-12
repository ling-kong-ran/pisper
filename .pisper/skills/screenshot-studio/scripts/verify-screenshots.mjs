// Verify the captured screenshots exist at 2558x1380, then replace docs/shots/.
import { copyFileSync, statSync } from 'node:fs'
import { resolve } from 'node:path'
import { spawnSync } from 'node:child_process'

const ROOT = resolve(import.meta.dirname, '../../../..')
const RUN_DIR = resolve(ROOT, 'generated/screenshot-run')
const SHOTS_DIR = resolve(ROOT, 'docs/shots')

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
  'skills',
  'welcome-dark',
  'workflow-builder',
  'workflows',
]

// Read PNG dimensions via PowerShell (System.Drawing) — zero new dependencies.
function dimensions(file) {
  const script = `Add-Type -AssemblyName System.Drawing; $img=[System.Drawing.Image]::FromFile('${file}'); Write-Output ($img.Width.ToString()+'x'+$img.Height.ToString()); $img.Dispose()`
  const result = spawnSync('powershell', ['-NoProfile', '-Command', script], {
    encoding: 'utf8',
    timeout: 30_000,
  })
  if (result.status !== 0) throw new Error(`dimension probe failed: ${result.stderr || result.stdout}`)
  return result.stdout.trim()
}

let failed = false
for (const name of WEB_SHOTS) {
  const source = resolve(RUN_DIR, `${name}.png`)
  try {
    statSync(source)
  } catch {
    console.error(`MISSING ${name}.png`)
    failed = true
    continue
  }
  const size = dimensions(source)
  if (size !== '2558x1380') {
    console.error(`BAD SIZE ${name}.png -> ${size} (expected 2558x1380)`)
    failed = true
    continue
  }
  copyFileSync(source, resolve(SHOTS_DIR, `${name}.png`))
  console.log(`replaced docs/shots/${name}.png (${size})`)
}

if (failed) {
  console.error('One or more screenshots failed verification; nothing else was touched.')
  process.exit(1)
}
console.log('All Web screenshots verified and replaced.')
