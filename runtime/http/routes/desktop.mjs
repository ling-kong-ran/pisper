// 桌面路由：应用更新检查、赞助内容、桌面宠物皮肤等桌面专属 API。
function requireDesktopPet(services) {
  if (!services.desktopPet) throw new Error('桌面宠物服务尚未初始化。')
  return services.desktopPet
}

// 截图以 base64 放在结果里，4K 整屏也不到这个数。
const MAX_BROWSER_RESULT_BYTES = 48_000_000

// 桌面前端连上来接收指令。远程客户端不能接：指令里有 Agent 要在本机输入的内容，
// 而且它操作不到这台机器上的网页和屏幕。
function desktopHostRoutes(path, channelOf) {
  return [
    {
      method: 'GET',
      path,
      async handler({ runtime, req, res, json, startSse, sendSse }) {
        if (req.pisperRemote) {
          json(403, { error: '远程客户端不能接收桌面指令。' })
          return
        }
        startSse()
        const detach = channelOf(runtime).attach((event, data) => sendSse(event, data))
        sendSse('ready', {})
        await new Promise((resolve) => res.once('close', resolve))
        detach()
      },
    },
    {
      method: 'POST',
      path: `${path}/:requestId`,
      async handler({ runtime, req, params, bodyBuffer, json }) {
        if (req.pisperRemote) {
          json(403, { error: '远程客户端不能接收桌面指令。' })
          return
        }
        const payload = JSON.parse((await bodyBuffer(MAX_BROWSER_RESULT_BYTES)).toString('utf8'))
        json(200, { accepted: channelOf(runtime).settle(params.requestId, payload) })
      },
    },
  ]
}

export const desktopRoutes = [
  ...desktopHostRoutes('/api/desktop-browser/host', (runtime) => runtime.desktopBrowserHost),
  ...desktopHostRoutes('/api/desktop-computer/host', (runtime) => runtime.desktopComputer),
  {
    method: 'POST',
    path: '/api/desktop/reveal-path',
    async handler({ runtime, req, body, json }) {
      if (req.pisperRemote) {
        json(403, { error: '远程客户端不能打开宿主文件管理器。' })
        return
      }
      const input = await body()
      json(200, await runtime.revealLocalPath(input?.path))
    },
  },
  {
    method: 'GET',
    path: '/api/app-update',
    async handler({ services, url, publicError, json }) {
      if (!services.updates) throw new Error('更新检查服务尚未初始化。')
      try {
        json(
          200,
          await services.updates.check({ refresh: url.searchParams.get('refresh') === '1' }),
        )
      } catch (error) {
        json(502, { error: publicError(error) })
      }
    },
  },
  {
    method: 'GET',
    path: '/api/desktop-pet',
    handler({ services, json }) {
      json(200, requireDesktopPet(services).status())
    },
  },
  {
    method: 'GET',
    path: '/api/desktop-pet/catalog',
    async handler({ services, url, json }) {
      json(200, await requireDesktopPet(services).search(url.searchParams.get('query') || ''))
    },
  },
  {
    method: 'POST',
    path: '/api/desktop-pet/enabled',
    async handler({ services, body, json }) {
      const desktopPet = requireDesktopPet(services)
      json(200, desktopPet.setEnabled(Boolean((await body())?.enabled)))
    },
  },
  {
    method: 'POST',
    path: '/api/desktop-pet/opacity',
    async handler({ services, body, json }) {
      const desktopPet = requireDesktopPet(services)
      json(200, desktopPet.setOpacity((await body())?.opacity))
    },
  },
  {
    method: 'POST',
    path: '/api/desktop-pet/install',
    async handler({ services, body, json }) {
      const desktopPet = requireDesktopPet(services)
      json(200, await desktopPet.install((await body())?.slug))
    },
  },
  {
    method: 'POST',
    path: '/api/desktop-pet/select',
    async handler({ services, body, json }) {
      const desktopPet = requireDesktopPet(services)
      json(200, desktopPet.select((await body())?.slug))
    },
  },
  {
    method: 'DELETE',
    path: '/api/desktop-pet/:slug',
    handler({ services, params, json }) {
      json(200, requireDesktopPet(services).remove(params.slug))
    },
  },
  {
    method: 'GET',
    path: '/api/desktop-pet/sprite',
    handler({ services, url, res, json }) {
      const sprite = requireDesktopPet(services).sprite(url.searchParams.get('slug') || '')
      if (!sprite) {
        json(404, { error: '宠物资源不存在。' })
        return
      }
      res.writeHead(200, {
        'Content-Type': sprite.mime,
        'Content-Length': sprite.buffer.length,
        'Cache-Control': 'private, max-age=86400',
        'X-Content-Type-Options': 'nosniff',
      })
      res.end(sprite.buffer)
    },
  },
]
