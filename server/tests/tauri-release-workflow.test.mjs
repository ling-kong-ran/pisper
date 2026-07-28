import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'

test('v0.4.0 alone republishes the archived Electron transition assets', async () => {
  const workflow = await readFile('.github/workflows/release.yml', 'utf8')
  const transitionStep = workflow.slice(
    workflow.indexOf('Stage Electron transition assets for v0.4.0'),
    workflow.indexOf('Create multi-platform updater manifest'),
  )

  assert.match(transitionStep, /if: github\.ref_name == 'v0\.4\.0'/)
  assert.match(transitionStep, /gh release download v0\.3\.3/)
  assert.match(transitionStep, /--dir artifacts\/electron-transition/)
  assert.doesNotMatch(workflow, /if: startsWith\(github\.ref_name, 'v0\.4'/)
})
