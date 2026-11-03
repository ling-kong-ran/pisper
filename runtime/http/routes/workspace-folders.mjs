import {
  createWorkspaceFolder,
  WorkspaceFolderError,
} from '../../services/workspace-folder-service.mjs'

// 新建项目文件夹：配对设备看不到主机的系统文件管理器，也要能就地建一个新项目目录，所以不限本机请求；
// 能派发任务的设备本来就能让代理在任意目录里写文件，这里只多一个受限的“建一层空文件夹”。
export const workspaceFolderRoutes = [
  {
    method: 'POST',
    path: '/api/workspace-folders',
    async handler({ runtime, body, req, json }) {
      // JSON 请求需要浏览器预检，跨站表单不能借本机页面在磁盘上建目录。
      if (!String(req.headers['content-type'] || '').startsWith('application/json')) {
        json(415, { error: '此操作需要 JSON 请求。', code: 'json_required' })
        return
      }
      const input = await body()
      try {
        const result = await createWorkspaceFolder(
          { parent: input?.parent, name: input?.name },
          { fallback: runtime.cwd || '' },
        )
        json(result.created ? 201 : 200, result)
      } catch (error) {
        // 结构化错误码交给客户端按语言显示；其他异常交给统一错误处理。
        if (!(error instanceof WorkspaceFolderError)) throw error
        json(error.statusCode, { error: error.message, code: error.code })
      }
    },
  },
]
