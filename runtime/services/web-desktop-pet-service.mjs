// 网页桌面宠物服务：管理宠物皮肤（从 petdex 拉取清单与素材）、
// 把 Agent 运行事件映射为宠物状态，供 Web 端与桌面端展示。
import {
  mkdirSync,
  readFileSync,
  readdirSync,
  realpathSync,
  statSync,
  unlinkSync,
  writeFileSync,
} from 'node:fs'
import { homedir } from 'node:os'
import { isAbsolute, join, relative } from 'node:path'
import {
  MAX_PET_BYTES,
  isPetSheetDimensions,
  normalizePetOpacity,
  petStateForAgentEvent,
  readImageDimensions,
} from '../../shared/desktop-pet-state.mjs'

const PETDEX_MANIFEST_URL = 'https://petdex.dev/api/manifest'
const PETDEX_PAGE_URL = 'https://petdex.dev'
const PET_SPRITE_NAMES = Object.freeze([
  'spritesheet.webp',
  'spritesheet.png',
  'sprite.webp',
  'sprite.png',
])
const SLUG_PATTERN = /^[a-z0-9][a-z0-9-]{0,79}$/
const MANIFEST_MAX_BYTES = 5 * 1024 * 1024

function safeJson(path, fallback) {
  try {
    return JSON.parse(readFileSync(path, 'utf8'))
  } catch {
    return fallback
  }
}

async function boundedFetch(fetchFn, value, maxBytes, allowedHost, redirectHosts = []) {
  const url = new URL(value)
  if (url.protocol !== 'https:' || url.hostname !== allowedHost)
    throw new Error('Petdex 资源地址不受信任。')
  const response = await fetchFn(url.href, {
    headers: { 'User-Agent': 'Pisper Web Desktop Pet' },
  })
  if (!response.ok) throw new Error(`Petdex request failed: HTTP ${response.status}`)
  const finalUrl = new URL(response.url || url.href)
  if (
    finalUrl.protocol !== 'https:' ||
    ![allowedHost, ...redirectHosts].includes(finalUrl.hostname)
  )
    throw new Error('Petdex 资源地址不受信任。')
  const declaredSize = Number(response.headers.get('content-length') || 0)
  if (declaredSize > maxBytes) throw new Error('宠物资源超过允许的大小。')
  const buffer = Buffer.from(await response.arrayBuffer())
  if (!buffer.length || buffer.length > maxBytes) throw new Error('宠物资源超过允许的大小。')
  return {
    buffer,
    contentType: String(response.headers.get('content-type') || '').toLowerCase(),
  }
}

export class WebDesktopPetService {
  constructor({ dataDir, fetchFn = globalThis.fetch } = {}) {
    this.dataDir = dataDir
    this.managedRoot = join(dataDir, 'desktop-pets')
    this.preferencesPath = join(dataDir, 'desktop-pet.json')
    this.fetchFn = fetchFn
    this.manifestCache = null
    this.manifestExpiresAt = 0
    this.state = 'idle'
    this.stateVersion = 0
    this.activeSessions = new Set()
    this.resetTimer = null
    mkdirSync(this.managedRoot, { recursive: true })
  }

  roots() {
    return [this.managedRoot, join(homedir(), '.petdex', 'pets'), join(homedir(), '.codex', 'pets')]
  }

  preferences() {
    const value = safeJson(this.preferencesPath, {})
    return {
      enabled: Boolean(value?.enabled),
      selectedSlug: typeof value?.selectedSlug === 'string' ? value.selectedSlug : '',
      opacity: normalizePetOpacity(value?.opacity),
    }
  }

  savePreferences(patch) {
    const next = { ...this.preferences(), ...patch }
    mkdirSync(this.dataDir, { recursive: true })
    writeFileSync(this.preferencesPath, `${JSON.stringify(next, null, 2)}\n`, 'utf8')
    return next
  }

  loadPet(root, slug) {
    let directory
    try {
      const entry = readdirSync(root, { withFileTypes: true }).find(
        (item) => item.isDirectory() && item.name === slug,
      )
      if (!entry) return null
      directory = join(root, entry.name)
    } catch {
      return null
    }

    for (const fileName of PET_SPRITE_NAMES) {
      const spritePath = join(directory, fileName)
      try {
        const rootPath = realpathSync(root)
        const resolvedPath = realpathSync(spritePath)
        const relativePath = relative(rootPath, resolvedPath)
        if (!relativePath || relativePath.startsWith('..') || isAbsolute(relativePath)) continue
        const size = statSync(resolvedPath).size
        if (size <= 0 || size > MAX_PET_BYTES) continue
        const buffer = readFileSync(resolvedPath)
        const image = readImageDimensions(buffer)
        if (!image || !isPetSheetDimensions(image)) continue
        const metadata = safeJson(join(directory, 'pet.json'), {})
        const name = String(metadata?.displayName || metadata?.name || slug).trim() || slug
        return { slug, name, buffer, path: resolvedPath, ...image }
      } catch {
        // Try the next supported sprite filename.
      }
    }
    return null
  }

  findPet(slug) {
    if (!SLUG_PATTERN.test(String(slug || ''))) return null
    for (const root of this.roots()) {
      const pet = this.loadPet(root, slug)
      if (pet) return { ...pet, source: root === this.managedRoot ? 'pisper' : 'petdex' }
    }
    return null
  }

  installedPets() {
    const pets = []
    const seen = new Set()
    for (const root of this.roots()) {
      let slugs = []
      try {
        slugs = readdirSync(root, { withFileTypes: true })
          .filter((entry) => entry.isDirectory())
          .map((entry) => entry.name)
          .sort()
      } catch {
        continue
      }
      for (const slug of slugs) {
        if (seen.has(slug)) continue
        const pet = this.loadPet(root, slug)
        if (!pet) continue
        seen.add(slug)
        pets.push({
          slug: pet.slug,
          name: pet.name,
          source: root === this.managedRoot ? 'pisper' : 'petdex',
        })
      }
    }
    return pets
  }

  status() {
    const preferences = this.preferences()
    const installed = this.installedPets()
    const selected =
      installed.find((pet) => pet.slug === preferences.selectedSlug) || installed[0] || null
    const loaded = selected ? this.findPet(selected.slug) : null
    return {
      supported: true,
      enabled: preferences.enabled,
      running: Boolean(preferences.enabled && loaded),
      selectedSlug: selected?.slug || '',
      selectedName: selected?.name || '',
      installed,
      opacity: preferences.opacity,
      state: this.state,
      stateVersion: this.stateVersion,
      sheetWidth: loaded?.width || 0,
      sheetHeight: loaded?.height || 0,
      spriteUrl: loaded
        ? `/api/desktop-pet/sprite?slug=${encodeURIComponent(loaded.slug)}&v=${statSync(loaded.path).mtimeMs}`
        : '',
    }
  }

  setOpacity(value) {
    this.savePreferences({ opacity: normalizePetOpacity(value) })
    return this.status()
  }

  setEnabled(enabled) {
    const installed = this.installedPets()
    if (enabled && !installed.length) throw new Error('请先安装一只宠物。')
    const current = this.preferences()
    this.savePreferences({
      enabled: Boolean(enabled),
      selectedSlug: current.selectedSlug || installed[0]?.slug || '',
    })
    return this.status()
  }

  select(slug) {
    const pet = this.findPet(String(slug || ''))
    if (!pet) throw new Error('宠物尚未安装。')
    this.savePreferences({ selectedSlug: pet.slug })
    return this.status()
  }

  async manifest() {
    if (this.manifestCache && Date.now() < this.manifestExpiresAt) return this.manifestCache
    const { buffer } = await boundedFetch(
      this.fetchFn,
      PETDEX_MANIFEST_URL,
      MANIFEST_MAX_BYTES,
      'petdex.dev',
      ['assets.petdex.dev'],
    )
    const data = JSON.parse(buffer.toString('utf8'))
    const pets = Array.isArray(data?.pets) ? data.pets : []
    this.manifestCache = pets.slice(0, 5000).flatMap((pet) => {
      const slug = String(pet?.slug || '')
        .trim()
        .toLowerCase()
      const displayName = String(pet?.displayName || slug).trim()
      const spritesheetUrl = String(pet?.spritesheetUrl || '')
      if (!SLUG_PATTERN.test(slug)) return []
      try {
        const asset = new URL(spritesheetUrl)
        if (asset.protocol !== 'https:' || asset.hostname !== 'assets.petdex.dev') return []
      } catch {
        return []
      }
      return [{ slug, displayName: displayName || slug, spritesheetUrl }]
    })
    this.manifestExpiresAt = Date.now() + 5 * 60 * 1000
    return this.manifestCache
  }

  async search(query = '') {
    const needle = String(query || '')
      .trim()
      .toLowerCase()
    const manifest = await this.manifest()
    return manifest
      .filter(
        (pet) =>
          !needle || pet.slug.includes(needle) || pet.displayName.toLowerCase().includes(needle),
      )
      .slice(0, needle ? 40 : 12)
      .map(({ slug, displayName }) => ({ slug, displayName }))
  }

  async install(inputSlug) {
    const slug = String(inputSlug || '')
      .trim()
      .toLowerCase()
    if (!SLUG_PATTERN.test(slug)) throw new Error('宠物标识格式无效。')
    const manifest = await this.manifest()
    const entry = manifest.find((pet) => pet.slug === slug)
    if (!entry) throw new Error('Petdex 中未找到这只宠物。')
    const { buffer, contentType } = await boundedFetch(
      this.fetchFn,
      entry.spritesheetUrl,
      MAX_PET_BYTES,
      'assets.petdex.dev',
    )
    const image = readImageDimensions(buffer)
    if (!image || !isPetSheetDimensions(image)) throw new Error('宠物图集格式无效。')
    if (contentType && !contentType.startsWith(image.mime)) throw new Error('宠物图集格式无效。')
    const directory = join(this.managedRoot, slug)
    mkdirSync(directory, { recursive: true })
    for (const fileName of PET_SPRITE_NAMES) {
      try {
        unlinkSync(join(directory, fileName))
      } catch {
        // Missing previous formats are expected.
      }
    }
    const extension = image.mime === 'image/png' ? 'png' : 'webp'
    writeFileSync(join(directory, `spritesheet.${extension}`), buffer)
    writeFileSync(
      join(directory, 'pet.json'),
      `${JSON.stringify({ id: slug, displayName: entry.displayName }, null, 2)}\n`,
      'utf8',
    )
    this.savePreferences({ selectedSlug: slug })
    return this.status()
  }

  sprite(slug) {
    const pet = this.findPet(String(slug || ''))
    if (!pet) return null
    return { buffer: pet.buffer, mime: pet.mime }
  }

  publishState(state, resetAfter = 0) {
    if (this.resetTimer) clearTimeout(this.resetTimer)
    this.resetTimer = null
    this.state = state
    this.stateVersion += 1
    if (resetAfter > 0) {
      this.resetTimer = setTimeout(() => {
        this.resetTimer = null
        this.state = this.activeSessions.size ? 'waiting' : 'idle'
        this.stateVersion += 1
      }, resetAfter)
      this.resetTimer.unref?.()
    }
  }

  observeRuntimeEvent({ event, sessionId } = {}) {
    const id = String(sessionId || '')
    if (event === 'done' || event === 'error') {
      if (id) this.activeSessions.delete(id)
      this.publishState(event === 'error' ? 'failed' : 'waving', event === 'error' ? 2200 : 1400)
      return
    }
    const state = petStateForAgentEvent(event)
    if (!state) return
    if (id) this.activeSessions.add(id)
    this.publishState(state)
  }

  dispose() {
    if (this.resetTimer) clearTimeout(this.resetTimer)
    this.resetTimer = null
    this.activeSessions.clear()
  }
}

export { PETDEX_PAGE_URL }
