import assert from 'node:assert/strict'
import test from 'node:test'
import { resolveGitCommit, UpdateCheckService } from '../services/update-check-service.mjs'

test('runtime update checks always compare the current commit with main', async () => {
  let requests = 0
  let now = Date.parse('2026-07-23T00:00:00.000Z')
  const service = new UpdateCheckService({
    currentVersion: '0.1.2',
    currentCommit: '1111111111111111111111111111111111111111',
    now: () => now,
    fetcher: async (_url, options) => {
      requests += 1
      assert.match(_url, /compare\/1111111111111111111111111111111111111111\.\.\.main$/)
      assert.equal(options.headers['User-Agent'], 'Pisper/0.1.2 (1111111)')
      assert.equal(options.headers['X-GitHub-Api-Version'], '2022-11-28')
      return {
        ok: true,
        json: async () => ({
          status: 'ahead',
          ahead_by: 1,
          html_url: 'https://github.com/ling-kong-ran/pisper/compare/1111111...main',
          commits: [
            {
              sha: '2222222222222222222222222222222222222222',
              commit: {
                message: 'feat: improve startup updates\n\nDetails',
                committer: { date: '2026-07-22T23:00:00.000Z' },
              },
            },
          ],
        }),
      }
    },
  })

  const first = await service.check()
  const second = await service.check()
  now += 2 * 60_000
  const third = await service.check()

  assert.equal(first.state, 'available')
  assert.equal(first.currentVersion, '0.1.2')
  assert.equal(first.currentCommit, '1111111111111111111111111111111111111111')
  assert.equal(first.availableCommit, '2222222222222222222222222222222222222222')
  assert.equal(first.behindBy, 1)
  assert.equal(first.branch, 'main')
  assert.equal(first.canDownload, false)
  assert.match(first.notes, /feat: improve startup updates \(2222222\)/)
  assert.notStrictEqual(second, first)
  assert.notStrictEqual(third, second)
  assert.equal(requests, 3)
})

test('current commit resolution prefers environment metadata and safely falls back to git', async () => {
  assert.equal(
    await resolveGitCommit('/workspace', {
      env: { PISPER_COMMIT_SHA: 'ABCDEF1234567' },
      runGit: async () => {
        throw new Error('should not run')
      },
    }),
    'abcdef1234567',
  )
  assert.equal(
    await resolveGitCommit('/workspace', {
      env: {},
      runGit: async () => ({ stdout: '1234567890abcdef\n' }),
    }),
    '1234567890abcdef',
  )
  assert.equal(
    await resolveGitCommit('/workspace', {
      env: {},
      runGit: async () => {
        throw new Error('missing git')
      },
    }),
    '',
  )
})
