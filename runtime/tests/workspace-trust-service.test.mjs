import assert from 'node:assert/strict'
import { mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import test from 'node:test'
import { WorkspaceTrustService } from '../services/workspace-trust-service.mjs'

test('workspace trust detects project resources and persists inherited decisions', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-workspace-trust-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const agentDir = join(directory, 'agent')
  const parent = join(directory, 'projects')
  const cwd = join(parent, 'demo')
  await mkdir(join(cwd, '.pisper', 'skills'), { recursive: true })
  await writeFile(join(cwd, '.pisper', 'skills', 'README.md'), 'project skill root', 'utf8')

  const service = new WorkspaceTrustService({ agentDir })
  assert.deepEqual(service.getStatus(cwd), {
    cwd: resolve(cwd),
    decision: null,
    trusted: false,
    restricted: true,
    requiresDecision: true,
    decisionPath: '',
    inherited: false,
    resources: ['skills'],
  })

  service.setTrusted(parent, true)
  const inherited = service.getStatus(cwd)
  assert.equal(inherited.trusted, true)
  assert.equal(inherited.requiresDecision, false)
  assert.equal(inherited.inherited, true)
  assert.equal(inherited.decisionPath, resolve(parent))

  const restricted = service.setTrusted(cwd, false)
  assert.equal(restricted.decision, false)
  assert.equal(restricted.restricted, true)
  assert.equal(restricted.inherited, false)
  assert.deepEqual(JSON.parse(await readFile(join(agentDir, 'trust.json'), 'utf8')), {
    [resolve(cwd)]: false,
    [resolve(parent)]: true,
  })
})

test('workspace trust ignores Pi project extensions while external Extensions are disabled', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-pi-trust-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const cwd = join(directory, 'workspace')
  await mkdir(join(cwd, '.pi', 'extensions'), { recursive: true })
  await writeFile(
    join(cwd, '.pi', 'extensions', 'unsafe.ts'),
    'throw new Error("not loaded")',
    'utf8',
  )

  const status = new WorkspaceTrustService({ agentDir: join(directory, 'agent') }).getStatus(cwd)
  assert.equal(status.requiresDecision, false)
  assert.equal(status.restricted, false)
  assert.equal(status.trusted, false)
  assert.deepEqual(status.resources, [])
})
