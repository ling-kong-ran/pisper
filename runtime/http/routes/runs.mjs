// Run 重挂路由：断线后按游标续传事件流。
// 帧格式与首次连接一致（run 头帧 → 业务帧带 id 游标），缓冲溢出时先插 resync_required。
import { sseSend } from '../response.mjs'
import { RunNotResumableError } from '../../services/run-registry.mjs'

export const runRoutes = [
  {
    method: 'GET',
    path: '/api/runs/:runId/events',
    async handler({ services, params, url, res, json, startSse }) {
      const after = Math.max(0, Number(url.searchParams.get('after') || 0) || 0)
      const registry = services.runs
      let prepared
      try {
        prepared = registry.prepareAttach(params.runId, after)
      } catch (error) {
        if (error instanceof RunNotResumableError) {
          json(409, { error: error.message, code: error.code })
          return
        }
        throw error
      }
      const { run, gap, replay } = prepared
      startSse()
      const write = (event, data, id = null) => {
        if (res.destroyed || res.writableEnded) return false
        const accepted = sseSend(res, event, data, id)
        if (!accepted) res.destroy?.()
        return accepted
      }
      // 头帧与首次连接一致，resumed 标记供客户端区分重挂场景。
      if (!write('run', { runId: run.id, ...run.meta, cursor: 0, resumed: true })) return
      if (gap && !write('resync_required', { reason: 'buffer_overflow', runId: run.id })) return
      for (const entry of replay) {
        if (!write(entry.event, entry.data, entry.cursor)) return
      }

      // run 已关闭：回放即全部内容，直接结束响应。
      const sink = {
        onEvent(cursor, event, data) {
          write(event, data, cursor)
        },
        onEnd: null,
      }
      await new Promise((resolve) => {
        let settled = false
        const finish = () => {
          if (settled) return
          settled = true
          registry.detach(run, sink)
          resolve()
        }
        sink.onEnd = finish
        // 客户端断开：解除订阅，run 本身继续缓存事件供后续重挂。
        res.on('close', finish)
        if (!registry.subscribe(run, sink)) finish()
      })
    },
  },
]
