// 冒烟夹具共用的样式服务：在随机回环端口起一个只用来提供 /src/index.css（Tailwind 编译）与
// 字体文件的 Vite 开发服务器。原先这些冒烟默认把样式请求放行到 127.0.0.1:5173，CI 上没有那个
// 开发服务器，本机上那又可能是别人正在用的服务；自带服务后两边都不依赖外部进程与网络。
import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import tailwindcss from '@tailwindcss/vite'
import { createServer } from 'vite'

export async function startSmokeStyleServer(root) {
  // 依赖预构建缓存放进临时目录，结束时连同目录一起删掉，不写进仓库的 node_modules/.vite。
  const cacheDir = await mkdtemp(join(tmpdir(), 'pisper-smoke-style-'))
  let server
  try {
    server = await createServer({
      root,
      configFile: false,
      cacheDir,
      logLevel: 'warn',
      plugins: [tailwindcss()],
      // 只提供样式与字体，不需要扫描或预构建任何 JS 依赖。
      optimizeDeps: { noDiscovery: true, include: [] },
      server: { host: '127.0.0.1', port: 0, hmr: false, watch: null },
      appType: 'custom',
    })
    await server.listen()
  } catch (error) {
    await server?.close().catch(() => {})
    await rm(cacheDir, { recursive: true, force: true })
    throw error
  }
  const { port } = server.httpServer.address()
  return {
    origin: `http://127.0.0.1:${port}`,
    close: async () => {
      try {
        await server.close()
      } finally {
        await rm(cacheDir, { recursive: true, force: true })
      }
    },
  }
}
