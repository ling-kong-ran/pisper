import { randomUUID } from 'node:crypto'

let config = {}
const sessions = new Map()
const send = (message) => {
  if (process.connected) process.send(message, () => {})
}
const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms))

process.once('disconnect', () => process.exit(0))
process.on('message', async (message) => {
  send({ event: 'call', method: message.method, params: message.params })
  if (message.method === 'shutdown') {
    setTimeout(() => process.exit(0), config.exitDelayMs || 0)
    return
  }
  const reply = (result) => send({ id: message.id, ok: true, result })
  const fail = () =>
    send({
      id: message.id,
      ok: false,
      error: { code: 'inference', message: '/private/model https://secret.invalid' },
    })
  const params = message.params || {}
  switch (message.method) {
    case 'init':
      config = params.model.config || {}
      if (config.initHang) return
      if (config.initError) return fail()
      await delay(config.initDelayMs || 0)
      reply({ ready: true })
      break
    case 'transcribe':
      if (params.samples[0] === -1) return
      if (params.samples[0] === -2) return process.exit(7)
      if (params.samples[0] === -3) return fail()
      await delay(config.delayMs || 0)
      reply(JSON.stringify({ pid: process.pid, samples: [...params.samples], terms: params.terms }))
      if (config.lateReply) setTimeout(() => reply('late'), 20)
      break
    case 'startSession': {
      await delay(config.delayMs || 0)
      const id = randomUUID()
      sessions.set(id, params.terms)
      reply({ id })
      break
    }
    case 'acceptChunk':
      if (params.samples[0] === -3) return fail()
      if (!sessions.has(params.id)) return fail()
      await delay(config.delayMs || 0)
      reply({ text: 'partial' })
      break
    case 'finishSession':
      if (!sessions.has(params.id)) return fail()
      await delay(config.delayMs || 0)
      sessions.delete(params.id)
      reply({ text: 'finished' })
      break
    case 'cancelSession':
      sessions.delete(params.id)
      reply({ ok: true })
      break
    case 'synthesize':
      if (params.text === 'hang') return
      if (params.text === 'crash') return process.exit(8)
      if (params.text === 'error') return fail()
      await delay(config.delayMs || 0)
      if (params.text === 'oversized')
        return reply({ wav: Buffer.alloc(46), sampleRate: 24000, durationMs: 46_000 })
      {
        const wav = Buffer.alloc(50)
        wav.write('RIFF', 0)
        wav.writeUInt32LE(42, 4)
        wav.write('WAVEfmt ', 8)
        wav.writeUInt32LE(16, 16)
        wav.writeUInt16LE(1, 20)
        wav.writeUInt16LE(1, 22)
        wav.writeUInt32LE(24000, 24)
        wav.writeUInt32LE(48000, 28)
        wav.writeUInt16LE(2, 32)
        wav.writeUInt16LE(16, 34)
        wav.write('data', 36)
        wav.writeUInt32LE(6, 40)
        if (params.text === 'bad-wav') wav.writeUInt16LE(2, 22)
        reply({ wav, sampleRate: 24000, durationMs: 0.125 })
      }
      break
    default:
      fail()
  }
})
