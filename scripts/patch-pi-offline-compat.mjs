// 集中适配 Pi 的隐式安装行为；升级依赖时必须重新核对源码与行为测试。
import { readFile, writeFile } from 'node:fs/promises'
import { join } from 'node:path'

export async function patchPiOfflineCompat(distRoot) {
  const patches = [
    {
      file: 'utils/tools-manager.js',
      before: 'import { join } from "path";',
      after: 'import { join } from "path";\nimport { fileURLToPath } from "node:url";',
    },
    {
      file: 'utils/tools-manager.js',
      before: '    // Check our tools directory first',
      after: `    // Pisper 随包工具优先于用户缓存，首次使用无需访问 GitHub。
    const bundledPath = fileURLToPath(new URL("../../vendor/bin/" + config.binaryName + (platform() === "win32" ? ".exe" : ""), import.meta.url));
    if (existsSync(bundledPath)) return bundledPath;
    // Check our tools directory first`,
    },
    {
      file: 'core/package-manager.js',
      // Pi 1.5 起没有 onMissing 时也走 try 安装、失败才跳过；这里在询问回调之前直接跳过。
      before: `                if (isOfflineModeEnabled())
                    return false;
                if (onMissing) {`,
      after: `                if (isOfflineModeEnabled())
                    return false;
                // Pisper 的资源发现不能隐式安装；显式安装入口保持原行为。
                if (!onMissing) return false;
                if (onMissing) {`,
    },
  ]
  for (const { file, before, after } of patches) {
    const path = join(distRoot, file)
    const source = await readFile(path, 'utf8')
    if (source.includes(after)) continue
    if (source.split(before).length !== 2) {
      throw new Error(`Pi offline compatibility patch no longer matches ${file}`)
    }
    await writeFile(path, source.replace(before, after))
  }
}
