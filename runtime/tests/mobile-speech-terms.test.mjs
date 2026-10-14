import assert from 'node:assert/strict'
import test from 'node:test'
import { createSpeechRecognizer } from '../../src/features/chat/voice-input.ts'

function fixture(t, response) {
  const previousWindow = globalThis.window
  const previousFetch = globalThis.fetch
  const calls = []
  globalThis.window = {
    __PISPER_MOBILE_APP__: true,
    __TAURI__: {
      core: {
        invoke: async (command, args) => {
          calls.push({ command, args })
          if (command === 'mobile_prepare_speech_session') return { ready: true }
          if (command === 'mobile_release_speech_session') return { released: true }
          return { text: 'use effect and py' }
        },
      },
    },
    clearInterval,
  }
  globalThis.fetch = async () => response
  t.after(() => {
    globalThis.fetch = previousFetch
    if (previousWindow === undefined) delete globalThis.window
    else globalThis.window = previousWindow
  })
  return calls
}

test('mobile speech requires the terms endpoint and never downgrades on 404', async (t) => {
  const calls = fixture(t, new Response('{"error":"not found"}', { status: 404 }))
  const recognizer = createSpeechRecognizer({ chatSessionId: 'current-session' })
  await assert.rejects(recognizer.start(), /not found/)
  assert.deepEqual(calls, [])
  await recognizer.dispose()
})

test('mobile speech sends context hotwords to native recognition and only formats known terms', async (t) => {
  const calls = fixture(t, Response.json({ terms: ['useEffect'] }))
  const recognizer = createSpeechRecognizer({ chatSessionId: 'current-session' })
  await recognizer.start()
  recognizer.acceptPcm(new Float32Array([0.1]))
  assert.equal(await recognizer.finish(), 'useEffect and py')
  assert.equal(
    calls.find((call) => call.command === 'mobile_prepare_speech_session').args.hotwords,
    'use effect',
  )
  assert.equal(
    calls.find((call) => call.command === 'mobile_transcribe_pcm').args.hotwords,
    'use effect',
  )
  await recognizer.dispose()
})

test('mobile speech does not hide authorization failures or malformed terminology', async (t) => {
  const calls = fixture(t, new Response('{"error":"unauthorized"}', { status: 401 }))
  const recognizer = createSpeechRecognizer()
  await assert.rejects(recognizer.start(), /unauthorized/)
  globalThis.fetch = async () => Response.json({ terms: [null] })
  await assert.rejects(recognizer.start(), /语音术语响应无效/)
  assert.deepEqual(calls, [])
  await recognizer.dispose()
})
