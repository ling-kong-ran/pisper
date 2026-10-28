import { isLocalAppRequest } from '../local-request.mjs'

// 系统文件夹选择框只为同机界面打开：配对设备看不到主机上的对话框，直接回报不可用以便退回手动输入。
export const directoryPickerRoutes = [
  {
    method: 'POST',
    path: '/api/local/directory-picker',
    async handler({ services, body, req, json }) {
      if (!isLocalAppRequest(req)) {
        json(200, { status: 'unavailable' })
        return
      }
      // JSON 请求需要浏览器预检，跨站表单不能借本机页面弹出系统对话框。
      if (!String(req.headers['content-type'] || '').startsWith('application/json')) {
        json(415, { error: '此操作需要 JSON 请求。', code: 'json_required' })
        return
      }
      const input = await body()
      json(
        200,
        await services.directoryPicker.pick({
          initialDirectory:
            typeof input?.initialDirectory === 'string' ? input.initialDirectory : '',
          title: typeof input?.title === 'string' ? input.title.slice(0, 200) : '',
        }),
      )
    },
  },
]
