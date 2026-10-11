import { execFileSync } from 'node:child_process'
import { mkdir, mkdtemp, readFile, readdir, rename, rm, stat, writeFile } from 'node:fs/promises'
import { dirname, join, resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import { gunzipSync } from 'node:zlib'
import tar from 'tar-stream'
import { parse as parseToml } from 'smol-toml'
import { download, sha256 } from './upstream-download.mjs'

export const PI_RUST_PIN = 'runtime-rs/pi-rs-source.json'
const INDEX = 'https://index.crates.io/pi/-r/pi-rs'
const stable = /^\d+\.\d+\.\d+$/
function compare(left, right) {
  const a = left.split('.').map(Number),
    b = right.split('.').map(Number)
  for (let i = 0; i < 3; i++) if (a[i] !== b[i]) return a[i] - b[i]
  return 0
}

export function latestStable(index, current) {
  const records = index
    .trim()
    .split('\n')
    .map((line) => JSON.parse(line))
  const candidates = records.filter(
    (entry) => entry.name === 'pi-rs' && stable.test(entry.vers) && entry.yanked === false,
  )
  for (const entry of candidates)
    if (!/^[a-f0-9]{64}$/.test(entry.cksum)) throw new Error('Invalid pi-rs registry checksum')
  const latest = candidates.sort((a, b) => compare(b.vers, a.vers))[0]
  if (!latest || compare(latest.vers, current) < 0)
    throw new Error('pi-rs registry is empty or older than the pinned source')
  return latest
}

export async function crateFiles(bytes, version) {
  const files = new Map()
  const extract = tar.extract()
  const content = gunzipSync(bytes, { maxOutputLength: 128 * 1024 * 1024 })
  await new Promise((resolveDone, reject) => {
    extract.on('error', reject)
    extract.on('finish', resolveDone)
    extract.on('entry', (header, stream, next) => {
      const prefix = `pi-rs-${version}/`
      const name = header.name.slice(prefix.length)
      if (
        !header.name.startsWith(prefix) ||
        name.includes('\\') ||
        name.split('/').includes('..') ||
        name.startsWith('/') ||
        name.includes('\0')
      ) {
        extract.destroy(new Error('Unsafe pi-rs archive path'))
        return
      }
      if (header.type === 'directory') {
        stream.resume()
        next()
        return
      }
      if (
        header.type !== 'file' ||
        !name ||
        files.has(name) ||
        header.size > 8 * 1024 * 1024 ||
        files.size >= 20000 ||
        name.split('/').some((part) => part === '.git' || part === 'target')
      ) {
        extract.destroy(new Error('Unsupported pi-rs archive entry'))
        return
      }
      const chunks = []
      stream.on('error', reject)
      stream.on('data', (chunk) => chunks.push(chunk))
      stream.on('end', () => {
        const bytes = Buffer.concat(chunks)
        bytes.fileMode = header.mode & 0o111 ? 0o755 : 0o644
        files.set(name, bytes)
        next()
      })
    })
    extract.end(content)
  })
  if (!files.has('Cargo.toml') || !files.has('LICENSE') || !files.has('src/lib.rs'))
    throw new Error('Incomplete pi-rs archive')
  const manifest = parseToml(files.get('Cargo.toml').toString('utf8'))
  if (manifest.package?.name !== 'pi-rs' || manifest.package?.version !== version)
    throw new Error('pi-rs archive version mismatch')
  if (manifest.package?.license !== 'MIT')
    throw new Error('pi-rs license changed; review is required before updating the source pin')
  return files
}

async function readTree(root, prefix = '') {
  const files = new Map()
  for (const entry of await readdir(join(root, prefix), { withFileTypes: true })) {
    if (['.git', 'target', '.cargo-ok'].includes(entry.name)) continue
    const relative = join(prefix, entry.name)
    if (entry.isSymbolicLink()) throw new Error('Symlink in local pi-rs source')
    if (entry.isDirectory()) for (const pair of await readTree(root, relative)) files.set(...pair)
    else {
      const bytes = await readFile(join(root, relative))
      bytes.fileMode = (await stat(join(root, relative))).mode & 0o111 ? 0o755 : 0o644
      files.set(relative.replaceAll('\\', '/'), bytes)
    }
  }
  return files
}

// 原版、Pisper 修订、新版之间逐文件三方合并；绝不靠覆盖目录丢弃本地补丁。
export async function mergePatchedFiles(base, local, upstream, temporary) {
  const merged = new Map(upstream)
  const same = (a, b) =>
    a === undefined
      ? b === undefined
      : b !== undefined && a.equals(b) && (a.fileMode ?? 0o644) === (b.fileMode ?? 0o644)
  for (const name of new Set([...base.keys(), ...local.keys()])) {
    const before = base.get(name),
      ours = local.get(name),
      theirs = upstream.get(name)
    if (same(before, ours)) continue
    if (same(theirs, before) || same(theirs, ours)) {
      if (ours === undefined) merged.delete(name)
      else merged.set(name, ours)
      continue
    }
    if (
      before === undefined ||
      ours === undefined ||
      theirs === undefined ||
      [before, ours, theirs].some((bytes) => bytes.includes(0))
    )
      throw new Error(`pi-rs patch conflict: ${name}`)
    const paths = ['upstream', 'base', 'pisper'].map((part) => join(temporary, part))
    await Promise.all(paths.map((path, i) => writeFile(path, [theirs, before, ours][i])))
    try {
      const result = execFileSync('git', ['merge-file', '-p', '--', ...paths], {
        maxBuffer: 16 * 1024 * 1024,
        stdio: ['ignore', 'pipe', 'pipe'],
      })
      result.fileMode = ours.fileMode !== before.fileMode ? ours.fileMode : theirs.fileMode
      merged.set(name, result)
    } catch {
      throw new Error(`pi-rs patch conflict: ${name}`)
    }
  }
  return merged
}

export async function updatePiRust({
  root = resolve(import.meta.dirname, '..'),
  fetchImpl = fetch,
  runCargo = (args) => execFileSync('cargo', args, { cwd: root, stdio: 'inherit' }),
} = {}) {
  const pinPath = join(root, PI_RUST_PIN)
  const pinBytes = await readFile(pinPath)
  const pin = JSON.parse(pinBytes)
  if (
    pin.schemaVersion !== 1 ||
    pin.crate !== 'pi-rs' ||
    !stable.test(pin.version) ||
    !/^[a-f0-9]{64}$/.test(pin.checksum)
  )
    throw new Error('Invalid pinned pi-rs provenance')
  const localManifest = parseToml(
    await readFile(join(root, 'runtime-rs/vendor/pi-rs/Cargo.toml'), 'utf8'),
  )
  if (localManifest.package?.name !== 'pi-rs' || localManifest.package?.version !== pin.version)
    throw new Error('Local pi-rs source does not match its provenance pin')
  const { bytes: index } = await download(INDEX, { fetchImpl, maxBytes: 4 * 1024 * 1024 })
  const latest = latestStable(index.toString('utf8'), pin.version)
  if (latest.vers === pin.version) {
    if (latest.cksum !== pin.checksum)
      throw new Error('Pinned pi-rs checksum differs from registry')
    return { changed: false, version: pin.version }
  }
  async function source(version, checksum) {
    const { bytes } = await download(
      `https://static.crates.io/crates/pi-rs/pi-rs-${version}.crate`,
      { fetchImpl, maxBytes: 64 * 1024 * 1024 },
    )
    if (sha256(bytes) !== checksum) throw new Error('pi-rs archive checksum mismatch')
    return crateFiles(bytes, version)
  }
  const [base, upstream] = await Promise.all([
    source(pin.version, pin.checksum),
    source(latest.vers, latest.cksum),
  ])
  const vendor = join(root, 'runtime-rs/vendor/pi-rs')
  const local = await readTree(vendor)
  await mkdir(join(root, 'release'), { recursive: true })
  const temporary = await mkdtemp(join(root, 'release/pi-rs-update-'))
  const newVendor = join(temporary, 'new-vendor'),
    backup = join(temporary, 'previous')
  const manifestPath = join(root, 'runtime-rs/Cargo.toml')
  const saved = new Map([
    [pinPath, pinBytes],
    [manifestPath, await readFile(manifestPath)],
  ])
  for (const path of ['runtime-rs/Cargo.lock', 'src-tauri/Cargo.lock'])
    saved.set(join(root, path), await readFile(join(root, path)))
  let replaced = false
  let preserveRecovery = false
  try {
    const merged = await mergePatchedFiles(base, local, upstream, temporary)
    for (const [name, bytes] of merged) {
      const path = join(newVendor, name)
      await mkdir(dirname(path), { recursive: true })
      await writeFile(path, bytes, { mode: bytes.fileMode ?? 0o644 })
    }
    const updated = saved
      .get(manifestPath)
      .toString('utf8')
      .replace(/^pi-rs = "[^"]+"$/m, `pi-rs = "=${latest.vers}"`)
    if (updated === saved.get(manifestPath).toString('utf8'))
      throw new Error('Cannot locate pi-rs dependency pin')
    await rename(vendor, backup)
    try {
      await rename(newVendor, vendor)
    } catch (error) {
      await rename(backup, vendor)
      throw error
    }
    replaced = true
    await writeFile(manifestPath, updated)
    for (const manifest of ['runtime-rs/Cargo.toml', 'src-tauri/Cargo.toml'])
      await runCargo(['update', '--manifest-path', manifest, '-p', 'pi-rs'])
    await writeFile(
      pinPath,
      `${JSON.stringify({ ...pin, version: latest.vers, checksum: latest.cksum }, null, 2)}\n`,
    )
    return { changed: true, version: latest.vers }
  } catch (error) {
    if (replaced) {
      try {
        await rm(vendor, { recursive: true })
        await rename(backup, vendor)
        for (const [path, bytes] of saved) await writeFile(path, bytes)
      } catch (rollback) {
        preserveRecovery = true
        throw new AggregateError(
          [error, rollback],
          'Pi update rollback failed; recovery data retained in release/pi-rs-update-*',
        )
      }
    }
    throw error
  } finally {
    if (!preserveRecovery) await rm(temporary, { recursive: true, force: true })
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const result = await updatePiRust()
  console.log(`${result.changed ? 'Updated' : 'Unchanged'} pi-rs: ${result.version}`)
}
