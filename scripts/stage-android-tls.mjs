import { execFileSync } from 'node:child_process'
import { createHash } from 'node:crypto'
import { mkdir, readFile, writeFile, copyFile } from 'node:fs/promises'
import { dirname, join } from 'node:path'

export const ANDROID_TLS_RESOURCE = Object.freeze({
  version: '0.1.1',
  source: 'https://github.com/rustls/rustls-platform-verifier',
  archive:
    'https://static.crates.io/crates/rustls-platform-verifier-android/rustls-platform-verifier-android-0.1.1.crate',
  archiveSha256: 'f87165f0995f63a9fbeea62b64d10b4d9d8e78ec6d7d51fb2125fda7bb36788f',
  bytes: 9287,
  sha256: '667292cadd8fa589229dd0f716541236a761f29b774930868d218175633830fd',
  license: 'MIT OR Apache-2.0',
})

export function validateAndroidTls(bytes) {
  if (
    bytes.length !== ANDROID_TLS_RESOURCE.bytes ||
    createHash('sha256').update(bytes).digest('hex') !== ANDROID_TLS_RESOURCE.sha256
  )
    throw new Error('Android TLS component digest mismatch')
}

export async function stageAndroidTls(root, appDirectory) {
  const metadata = JSON.parse(
    execFileSync(
      process.env.CARGO || 'cargo',
      [
        'metadata',
        '--locked',
        '--format-version',
        '1',
        '--filter-platform',
        'aarch64-linux-android',
        '--manifest-path',
        join(root, 'src-tauri/Cargo.toml'),
      ],
      { encoding: 'utf8', maxBuffer: 16 * 1024 * 1024 },
    ),
  )
  const native = metadata.packages.find((item) => item.name === 'rustls-platform-verifier-android')
  const verifier = metadata.packages.find((item) => item.name === 'rustls-platform-verifier')
  if (native?.version !== ANDROID_TLS_RESOURCE.version || !verifier)
    throw new Error('Android TLS resource must be reviewed when the Cargo verifier changes')
  // Cargo 按锁文件校验发布归档；AAR 再独立验摘要，禁止混用 ABI 版本。
  const bytes = await readFile(
    join(
      dirname(native.manifest_path),
      'maven/rustls/rustls-platform-verifier/0.1.1/rustls-platform-verifier-0.1.1.aar',
    ),
  )
  validateAndroidTls(bytes)
  await mkdir(join(appDirectory, 'libs'), { recursive: true })
  await writeFile(join(appDirectory, 'libs/rustls-platform-verifier.aar'), bytes)
  const notices = join(appDirectory, 'src/main/assets/licenses/rustls-platform-verifier')
  await mkdir(notices, { recursive: true })
  await writeFile(
    join(notices, 'provenance.json'),
    JSON.stringify(ANDROID_TLS_RESOURCE, null, 2) + '\n',
  )
  for (const name of ['LICENSE-MIT', 'LICENSE-APACHE'])
    await copyFile(join(dirname(verifier.manifest_path), name), join(notices, name))
}
