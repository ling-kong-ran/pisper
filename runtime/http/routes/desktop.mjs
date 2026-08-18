// 桌面路由：应用更新检查、赞助内容、桌面宠物皮肤等桌面专属 API。
function requireDesktopPet(services) {
  if (!services.desktopPet) throw new Error('桌面宠物服务尚未初始化。')
  return services.desktopPet
}

export const desktopRoutes = [
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
    path: '/api/sponsors/:placement',
    async handler({ services, params, url, json }) {
      if (!services.sponsors) throw new Error('赞助内容服务尚未初始化。')
      json(
        200,
        await services.sponsors.getPlacement(params.placement, {
          locale: url.searchParams.get('locale') || 'zh-CN',
          refresh: url.searchParams.get('refresh') === '1',
        }),
      )
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
