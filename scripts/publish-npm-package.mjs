import { createHash } from 'node:crypto'
import { readFile } from 'node:fs/promises'

// npm 接收上传与版本公开是两个阶段：扫描期间再次 PUT 同一版本会得到 409。
// 只在明确 404 时上传；上传后等待可读并核对内容，不能把网络错误当成版本不存在。
export async function publishNpmPackage({
  version,
  tarball,
  tag,
  userconfig,
  npm,
  sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms)),
  polls = 45,
  log = console.log,
}) {
  const spec = `pisper@${version}`
  const integrity = `sha512-${createHash('sha512')
    .update(await readFile(tarball))
    .digest('base64')}`
  async function inspect() {
    for (let attempt = 1; ; attempt += 1) {
      try {
        const value = JSON.parse(
          npm(
            [
              'view',
              spec,
              '--json',
              '--prefer-online',
              '--fetch-retries=0',
              '--fetch-timeout=30000',
            ],
            { capture: true },
          ),
        )
        if (value.version !== version || value.dist?.integrity !== integrity)
          throw new Error(`${spec} 的公开内容与本次构建不一致，停止发布。`)
        return value
      } catch (error) {
        const output = `${error.stderr || ''}\n${error.stdout || ''}`
        if (/\bE404\b/.test(output)) return null
        if (
          !/\b(EAI_AGAIN|ECONNRESET|ETIMEDOUT|ECONNREFUSED|E5\d\d)\b/.test(output) ||
          attempt >= 4
        )
          throw error
        await sleep(attempt * 2_000)
      }
    }
  }

  let metadata = await inspect()
  if (!metadata) {
    try {
      npm(
        [
          'publish',
          tarball,
          '--access',
          'public',
          '--no-provenance',
          '--tag',
          tag,
          '--userconfig',
          userconfig,
        ],
        { capture: true },
      )
    } catch (error) {
      const output = `${error.stderr || ''}\n${error.stdout || ''}`
      // 只协调确定的暂存冲突，权限、身份校验和其他发布失败继续报错。
      if (!/\bE409\b/.test(output) || !/previously staged version/.test(output)) throw error
      log(`${spec} 已被 npm 暂存，等待服务器处理，不重复上传。`)
    }
    for (let poll = 0; poll < polls; poll += 1) {
      metadata = await inspect()
      if (metadata) break
      if (poll % 3 === 0) log(`等待 ${spec} 在 npm 公开（${poll + 1}/${polls}）。`)
      await sleep(20_000)
    }
    if (!metadata)
      throw new Error(
        `${spec} 尚未公开：请检查 npm 的扫描或暂存审批状态，完成后重试原 Publish 作业。发布计划已保留。`,
      )
  }
  if (metadata['dist-tags']?.[tag] !== version) {
    npm(['dist-tag', 'add', spec, tag, '--userconfig', userconfig], { capture: true })
    metadata = await inspect()
    if (metadata?.['dist-tags']?.[tag] !== version)
      throw new Error(`${spec} 已公开，但 ${tag} 标签尚未确认，请重试原 Publish 作业。`)
  }
  log(`已验证 ${spec} 的公开版本、摘要和 ${tag} 标签。`)
}
