import { createHash } from 'node:crypto'
import { readFile, readdir, rm, writeFile } from 'node:fs/promises'
import { join } from 'node:path'
import { create as createTar } from 'tar'

const manifestName = 'embedded-runtime.json'

// 显式排序且禁止 tar 再次递归，避免文件系统枚举顺序和 mtime 改变相同闭包的指纹。
async function archiveEntries(directory, prefix = '') {
  const entries = await readdir(directory, { withFileTypes: true })
  entries.sort((left, right) => (left.name < right.name ? -1 : left.name > right.name ? 1 : 0))
  const paths = []
  for (const entry of entries) {
    const path = prefix ? `${prefix}/${entry.name}` : entry.name
    if (path === manifestName) continue
    paths.push(path)
    if (entry.isDirectory())
      paths.push(...(await archiveEntries(join(directory, entry.name), path)))
  }
  return paths
}

export async function createMobileRuntimeArchive({
  runtimeDir,
  output,
  appVersion,
  runtimeProfile,
}) {
  if (!appVersion || !['mobile-embedded', 'mobile-store'].includes(runtimeProfile))
    throw new Error('Invalid mobile Runtime App version or profile')
  const entry = 'runtime/mobile-embedded.mjs'
  const fingerprint = async (path) => {
    const contents = await readFile(join(runtimeDir, path))
    if (!contents.length) throw new Error(`Empty mobile Runtime entry: ${path}`)
    return createHash('sha256').update(contents).digest('hex')
  }
  const embeddedManifest = {
    schemaVersion: 1,
    appVersion,
    runtimeProfile,
    entry,
    entrySha256: await fingerprint(entry),
    frontendSha256: await fingerprint('dist/index.html'),
  }
  const entries = await archiveEntries(runtimeDir)
  const tarOptions = {
    cwd: runtimeDir,
    portable: true,
    noMtime: true,
    noDirRecurse: true,
    prefix: '.',
  }
  // 指纹覆盖清单合同和完整闭包，但排除上次生成的清单，避免摘要自引用。
  const digest = createHash('sha256').update(JSON.stringify(embeddedManifest)).update('\n')
  for await (const chunk of createTar(tarOptions, entries)) digest.update(chunk)
  const manifest = { ...embeddedManifest, buildSha256: digest.digest('hex') }
  const serialized = `${JSON.stringify(manifest, null, 2)}\n`
  if (Buffer.byteLength(serialized) > 8 * 1024)
    throw new Error('Mobile Runtime manifest exceeds the native probe limit')
  await writeFile(join(runtimeDir, manifestName), serialized)
  await rm(output, { force: true })
  // 清单位于首项，让两端读取受 App 签名保护的指纹；首次安装仍完整 hash 并解压归档。
  await createTar({ ...tarOptions, file: output, gzip: true }, [manifestName, ...entries])
  return manifest
}
