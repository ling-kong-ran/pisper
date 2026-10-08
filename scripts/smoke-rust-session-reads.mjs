import assert from 'node:assert/strict'
import { createHash, randomUUID } from 'node:crypto'
import { readFile, readdir } from 'node:fs/promises'
import { join, resolve } from 'node:path'

// Seven empty journals exceed the resident-runtime limit. Projection reads must
// preserve bytes even when the session has never been hosted or was evicted.
export async function checkSessionProjectionReads({ check, json, request, workspace, agent }) {
  await check(
    'Goal, Plan, Agents and Team GET preserve seven original empty journals byte-for-byte',
    async () => {
      const ids = []
      const root = join(resolve(agent), 'sessions')
      async function journals(directory) {
        const files = []
        for (const entry of await readdir(directory, { withFileTypes: true })) {
          const path = join(directory, entry.name)
          if (entry.isDirectory()) files.push(...(await journals(path)))
          else if (entry.isFile() && entry.name.endsWith('.jsonl')) files.push(path)
        }
        return files
      }
      const sha = (bytes) => createHash('sha256').update(bytes).digest('hex')
      try {
        for (let index = 0; index < 7; index++) {
          const value = await json('/api/sessions', 'POST', {
            name: `read-projection-${randomUUID()}`,
            cwd: resolve(workspace),
          })
          assert.ok(typeof value.id === 'string' && value.id.length > 0)
          ids.push(value.id)
        }
        const original = new Map()
        for (const file of await journals(root)) {
          const bytes = await readFile(file)
          const header = JSON.parse(bytes.toString('utf8').split(/\r?\n/, 1)[0])
          if (ids.includes(header.id)) original.set(header.id, { file, hash: sha(bytes) })
        }
        assert.equal(original.size, 7)
        for (let round = 0; round < 2; round++) {
          for (const id of ids) {
            for (const [route, status, code] of [
              ['agents', 200],
              ['plan', 200],
              ['goal', 404, 'goal_not_found'],
              ['team', 404, 'team_not_found'],
            ]) {
              const response = await request(`/api/sessions/${encodeURIComponent(id)}/${route}`)
              const value = await response.json()
              assert.equal(response.status, status, `Projection ${route} HTTP status`)
              if (code) assert.equal(value.code, code)
              if (route === 'agents') assert.deepEqual(value.agents, [])
            }
          }
        }
        for (const { file, hash } of original.values()) {
          assert.equal(
            sha(await readFile(file)),
            hash,
            'Projection GET must never append model metadata',
          )
        }
        return { sessions: original.size, rounds: 2, projections: 4, exactOriginalBytes: true }
      } finally {
        // Only delete session identities created by this isolated helper.
        for (const id of ids) await json(`/api/sessions/${encodeURIComponent(id)}`, 'DELETE')
      }
    },
  )
}
