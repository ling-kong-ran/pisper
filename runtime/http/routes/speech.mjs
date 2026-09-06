function requireModels(services) {
  if (!services.speechModels || !services.speechCatalog) {
    throw Object.assign(new Error('Local speech models are unavailable.'), { statusCode: 503 })
  }
}

function fields(value, allowed) {
  if (
    !value ||
    typeof value !== 'object' ||
    Array.isArray(value) ||
    Object.keys(value).some((key) => !allowed.includes(key))
  ) {
    throw new Error('Invalid speech request.')
  }
  return value
}

function publicModel(model, state) {
  return {
    id: model.id,
    kind: model.kind,
    engine: model.engine,
    name: model.name,
    languages: model.languages,
    license: model.license,
    ...(model.voices
      ? { voices: model.voices.map(({ id, name, language }) => ({ id, name, language })) }
      : {}),
    status: state.status,
    downloadedBytes: state.downloadedBytes,
    totalBytes: state.totalBytes,
    filesBytes: model.files.reduce((bytes, file) => bytes + file.bytes, 0),
    ...(state.error ? { error: state.error } : {}),
  }
}

export const speechRoutes = [
  {
    method: 'GET',
    path: '/api/speech/models',
    async handler({ services, json }) {
      requireModels(services)
      const states = new Map((await services.speechModels.list()).map((state) => [state.id, state]))
      json(200, {
        defaults: services.speechCatalog.defaults,
        models: services.speechCatalog.models.map((model) =>
          publicModel(model, states.get(model.id)),
        ),
      })
    },
  },
  {
    method: 'POST',
    path: '/api/speech/models/download',
    async handler({ services, body, json }) {
      requireModels(services)
      const { modelId } = fields(await body(), ['modelId'])
      const state = await services.speechModels.startDownload(modelId)
      json(
        200,
        publicModel(
          services.speechCatalog.models.find((model) => model.id === modelId),
          state,
        ),
      )
    },
  },
  {
    method: 'POST',
    path: '/api/speech/models/cancel',
    async handler({ services, body, json }) {
      requireModels(services)
      const { modelId } = fields(await body(), ['modelId'])
      const state = await services.speechModels.cancelDownload(modelId)
      json(
        200,
        publicModel(
          services.speechCatalog.models.find((model) => model.id === modelId),
          state,
        ),
      )
    },
  },
  {
    method: 'POST',
    path: '/api/speech/session',
    async handler({ services, req, res, body }) {
      if (!services.speech)
        throw Object.assign(new Error('Local speech synthesis is unavailable.'), {
          statusCode: 503,
        })
      const input = fields(await body(), ['requestId', 'kinds', 'hotwords', 'voiceId'])
      const controller = new AbortController()
      let heartbeat
      let finish
      const closed = new Promise((resolve) => {
        finish = resolve
      })
      const close = () => {
        controller.abort()
        finish()
      }
      req.once('aborted', close)
      res.once('close', close)
      try {
        if (req.aborted || res.destroyed) return
        await services.speech.prepareSpeechSession(input, controller.signal)
        if (controller.signal.aborted || res.destroyed) return
        res.writeHead(200, {
          'Content-Type': 'text/event-stream; charset=utf-8',
          'Cache-Control': 'no-store',
          'X-Content-Type-Options': 'nosniff',
        })
        res.write('event: ready\ndata: {"ready":true}\n\n')
        // 由连接生命周期持有模型，页面关闭或网络断开就释放，不依赖前端最后一次通知。
        heartbeat = setInterval(() => {
          if (!res.destroyed) res.write(': speech-session\n\n')
        }, 15_000)
        heartbeat.unref?.()
        await closed
      } catch (error) {
        if (!controller.signal.aborted && !res.destroyed) throw error
      } finally {
        clearInterval(heartbeat)
        close()
        req.removeListener('aborted', close)
        res.removeListener('close', close)
      }
    },
  },
  {
    method: 'POST',
    path: '/api/speech/synthesize',
    async handler({ services, req, res, body }) {
      if (!services.speech)
        throw Object.assign(new Error('Local speech synthesis is unavailable.'), {
          statusCode: 503,
        })
      const input = fields(await body(), ['text', 'voiceId', 'requestId'])
      const cancel = () => {
        void services.speech.cancelSpeech(input.requestId).catch(() => {})
      }
      req.once('aborted', cancel)
      res.once('close', cancel)
      try {
        if (req.aborted || res.destroyed) return
        const result = await services.speech.synthesize(input)
        if (req.aborted || res.destroyed) return
        res.writeHead(200, {
          'Content-Type': 'audio/wav',
          'Content-Length': result.wav.length,
          'Cache-Control': 'no-store',
          'X-Content-Type-Options': 'nosniff',
        })
        res.end(result.wav)
      } catch (error) {
        if (!req.aborted && !res.destroyed) throw error
      } finally {
        req.removeListener('aborted', cancel)
        res.removeListener('close', cancel)
      }
    },
  },
  {
    method: 'POST',
    path: '/api/speech/cancel',
    async handler({ services, body, json }) {
      if (!services.speech)
        throw Object.assign(new Error('Local speech synthesis is unavailable.'), {
          statusCode: 503,
        })
      const { requestId } = fields(await body(), ['requestId'])
      json(200, await services.speech.cancelSpeech(requestId))
    },
  },
]
