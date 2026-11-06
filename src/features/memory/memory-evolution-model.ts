// 记忆年轮的纯逻辑：把 /api/memory/evolution 的数据算成一个“场景”，再回答“时刻 T 时每条记忆处于什么状态”。
// 位置只由数据决定，和 T 无关：同一条事实不管时间轴拖到哪都在同一个地方，时间演算才看得出“生长”。
// T 表示“系统在那一刻知道什么”：事后才知道的事，到知道的那一刻才亮，但位置落在它真正发生的那一圈。
// 全部结果放进定型数组：渲染每帧只做比较，不分配内存。
import type {
  MemoryEvolutionData,
  MemoryEvolutionFact,
  MemorySpace,
  MemoryType,
} from './memory-types'

const DAY_MS = 86_400_000
const TAU = Math.PI * 2
// 圆心留一点空间给最早的“髓心”。
const CORE_RHO = 0.055
// 小于 1：早年的圈不至于挤成一团。
const RADIAL_POWER = 0.85

export const SLIDER_MAX = 1000
// 再小看不清结构，再大只剩几个光点。
export const EVOLUTION_MIN_ZOOM = 0.72
export const EVOLUTION_MAX_ZOOM = 2.6
// 窄屏放不下一个看得清的圆盘，默认放大，把画幅留给最新的外圈。
export const EVOLUTION_NARROW_ZOOM = 1.75
// 记忆不多时不挤，放大反而把它们裁到画面外，还是看整个圆盘。
const NARROW_ZOOM_MIN_FACTS = 30
const EVOLUTION_SELECTED_ZOOM = 1.1
// 最老的记忆暗到这个比例。太低就看不出那里还有东西，时间演算也就没了来处。
const OLDEST_FADE = 0.34
export const OPENING_DURATION_MS = 6000

export const EVOLUTION_TYPES: readonly MemoryType[] = [
  'decision',
  'preference',
  'fact',
  'concept',
  'risk',
  'file',
  'task',
]

export const FACT_HIDDEN = 0
export const FACT_ACTIVE = 1
export const FACT_SUPERSEDED = 2
export const FACT_ARCHIVED = 3
export type FactStateCode = 0 | 1 | 2 | 3
export type FactStateName = 'hidden' | 'born' | 'active' | 'superseded' | 'archived'

export const FLAG_PINNED = 1
export const FLAG_TAINTED = 2
export const FLAG_LATE = 4
export const FLAG_STALE = 8

export type EvolutionDomain = { start: number; end: number }
export type EvolutionRingUnit = 'day' | 'week' | 'month' | 'quarter' | 'year'
export type EvolutionRing = {
  at: number
  rho: number
  major: boolean
  // 上一圈到这一圈之间发生的事占最多那一圈的比例：活动越多的圈线条越亮，像真实年轮的深浅。
  activity: number
}
export type EvolutionSector = {
  key: string
  label: string
  kind: 'topic' | 'type'
  spaceId: string
  start: number
  end: number
  count: number
}
export type EvolutionSpaceArc = { spaceId: string; name: string; start: number; end: number }
export type EvolutionDigestArc = {
  id: string
  level: string
  at: number
  rho: number
  start: number
  end: number
}
export type EvolutionReplacement = { at: number; from: number; to: number }
export type EvolutionStats = { active: number; superseded: number; archived: number; known: number }

export type EvolutionScene = {
  facts: MemoryEvolutionFact[]
  count: number
  indexById: Map<string, number>
  domain: EvolutionDomain
  unit: EvolutionRingUnit
  rings: EvolutionRing[]
  // 刚诞生的闪光持续多久（时刻长度），也是纯逻辑里 born 状态的宽度。
  bornWindow: number
  validAt: Float64Array
  appearAt: Float64Array
  replacedAt: Float64Array
  archivedAt: Float64Array
  theta: Float32Array
  rho: Float32Array
  x: Float32Array
  y: Float32Array
  lift: Float32Array
  size: Float32Array
  brightness: Float32Array
  phase: Float32Array
  typeIndex: Uint8Array
  sector: Int16Array
  flags: Uint8Array
  successor: Int32Array
  predecessors: Map<number, number[]>
  replacements: EvolutionReplacement[]
  sectors: EvolutionSector[]
  spaceArcs: EvolutionSpaceArc[]
  digests: EvolutionDigestArc[]
  histogram: Uint16Array
  histogramReplacements: Uint16Array
  truncated: boolean
  sortedAppear: Float64Array
  sortedEnded: Float64Array
  sortedArchived: Float64Array
}

// 稳定哈希（FNV-1a）：角度和相位只由 id 决定，换机器、换顺序都不变。
export function stableHash(value: string) {
  let hash = 2166136261
  for (let index = 0; index < value.length; index += 1) {
    hash ^= value.charCodeAt(index)
    hash = Math.imul(hash, 16777619)
  }
  return hash >>> 0
}

const hash01 = (value: string) => stableHash(value) / 4294967296
const clamp01 = (value: number) => Math.min(1, Math.max(0, value))

function parseTime(value: string | undefined | null, fallback: number) {
  const time = Date.parse(String(value || ''))
  return Number.isNaN(time) ? fallback : time
}

export function rhoAt(domain: EvolutionDomain, at: number) {
  const span = Math.max(1, domain.end - domain.start)
  return CORE_RHO + (1 - CORE_RHO) * clamp01((at - domain.start) / span) ** RADIAL_POWER
}

// 年轮不是正圆：低频起伏随半径缓慢变化，相邻两圈形状相近；髓心略微偏离几何中心，越往外越居中。
// 只用常数相位，不依赖数据，同一位置的起伏永远一样。
export function warpRadius(theta: number, rho: number) {
  const wave =
    0.62 * Math.sin(3 * theta + 1.1 + 1.9 * rho) +
    0.28 * Math.sin(5 * theta + 4.2 - 2.7 * rho) +
    0.1 * Math.sin(11 * theta + 2.3 + 5.1 * rho)
  return rho * (1 + 0.046 * wave * Math.min(1, rho * 2.2))
}

const ECCENTRIC_X = 0.042
const ECCENTRIC_Y = -0.03

export function discX(theta: number, rho: number) {
  return warpRadius(theta, rho) * Math.cos(theta) + ECCENTRIC_X * (1 - rho)
}

export function discY(theta: number, rho: number) {
  return warpRadius(theta, rho) * Math.sin(theta) + ECCENTRIC_Y * (1 - rho)
}

export function chooseRingUnit(domain: EvolutionDomain): EvolutionRingUnit {
  const days = (domain.end - domain.start) / DAY_MS
  if (days <= 21) return 'day'
  if (days <= 120) return 'week'
  if (days <= 1000) return 'month'
  if (days <= 3000) return 'quarter'
  return 'year'
}

function firstBoundary(start: number, unit: EvolutionRingUnit) {
  const date = new Date(start)
  if (unit === 'day') return new Date(date.getFullYear(), date.getMonth(), date.getDate() + 1)
  if (unit === 'week') {
    const toMonday = (8 - date.getDay()) % 7 || 7
    return new Date(date.getFullYear(), date.getMonth(), date.getDate() + toMonday)
  }
  if (unit === 'month') return new Date(date.getFullYear(), date.getMonth() + 1, 1)
  if (unit === 'quarter')
    return new Date(date.getFullYear(), Math.floor(date.getMonth() / 3) * 3 + 3, 1)
  return new Date(date.getFullYear() + 1, 0, 1)
}

function nextBoundary(date: Date, unit: EvolutionRingUnit) {
  if (unit === 'day') return new Date(date.getFullYear(), date.getMonth(), date.getDate() + 1)
  if (unit === 'week') return new Date(date.getFullYear(), date.getMonth(), date.getDate() + 7)
  if (unit === 'month') return new Date(date.getFullYear(), date.getMonth() + 1, 1)
  if (unit === 'quarter') return new Date(date.getFullYear(), date.getMonth() + 3, 1)
  return new Date(date.getFullYear() + 1, 0, 1)
}

function isMajorBoundary(date: Date, unit: EvolutionRingUnit) {
  if (unit === 'day') return date.getDay() === 1
  if (unit === 'week') return date.getDate() <= 7
  if (unit === 'month') return date.getMonth() % 3 === 0
  if (unit === 'quarter') return date.getMonth() === 0
  return true
}

export function buildRings(domain: EvolutionDomain, unit: EvolutionRingUnit, appears: number[]) {
  const boundaries: Date[] = []
  let cursor = firstBoundary(domain.start, unit)
  while (cursor.getTime() < domain.end && boundaries.length < 400) {
    boundaries.push(cursor)
    cursor = nextBoundary(cursor, unit)
  }
  const counts = new Array<number>(boundaries.length).fill(0)
  for (const at of appears) {
    let low = 0
    let high = boundaries.length
    while (low < high) {
      const middle = (low + high) >> 1
      if (boundaries[middle].getTime() < at) low = middle + 1
      else high = middle
    }
    if (low < counts.length) counts[low] += 1
  }
  const peak = Math.max(1, ...counts)
  return boundaries.map((date, index): EvolutionRing => {
    const at = date.getTime()
    return {
      at,
      rho: rhoAt(domain, at),
      major: isMajorBoundary(date, unit),
      activity: counts[index] / peak,
    }
  })
}

// 主题键的前几段常常只是“project”“user”这样的大类，取第一个有区分度的段当聚类。
const GENERIC_TOPIC_SEGMENTS = new Set([
  'project',
  'projects',
  'user',
  'users',
  'global',
  'repo',
  'app',
  'pisper',
  'general',
  'misc',
  'title',
])

export function topicClusterOf(topicKey: string) {
  const segments = String(topicKey || '')
    .split('.')
    .filter(Boolean)
  while (segments.length > 1 && GENERIC_TOPIC_SEGMENTS.has(segments[0])) segments.shift()
  return segments[0] || ''
}

function parseFactTimes(fact: MemoryEvolutionFact, fallback: number) {
  const created = parseTime(fact.createdAt, fallback)
  const valid = parseTime(fact.validFrom, created)
  return { created, valid, appear: Math.max(created, valid) }
}

export function computeDomain(facts: readonly MemoryEvolutionFact[], now: number): EvolutionDomain {
  if (!facts.length) return { start: now - 180 * DAY_MS, end: now }
  let earliest = Infinity
  let latest = now
  for (const fact of facts) {
    const { valid, appear } = parseFactTimes(fact, now)
    earliest = Math.min(earliest, valid, appear)
    if (appear <= now) latest = Math.max(latest, appear)
  }
  const span = Math.max(DAY_MS, latest - earliest)
  // 前面留一点空白：开场时圆心先亮，第一条记忆再出现。太短的跨度也撑到一周，圈才看得出来。
  const start = Math.min(earliest - Math.max(span * 0.02, DAY_MS / 4), latest - 7 * DAY_MS)
  return { start, end: latest }
}

type SectorPlan = {
  key: string
  label: string
  kind: 'topic' | 'type'
  spaceId: string
  members: number[]
}

function planSectors(facts: readonly MemoryEvolutionFact[]) {
  const clusters = new Map<string, number[]>()
  const clusterKeys = facts.map((fact) => topicClusterOf(fact.topicKey))
  facts.forEach((fact, index) => {
    const key = `${fact.spaceId}\u0000${clusterKeys[index]}`
    const list = clusters.get(key)
    if (list) list.push(index)
    else clusters.set(key, [index])
  })
  const threshold = Math.max(2, Math.ceil(facts.length * 0.012))
  const bySpace = new Map<string, SectorPlan[]>()
  const addTo = (spaceId: string, plan: SectorPlan) => {
    const list = bySpace.get(spaceId)
    if (list) list.push(plan)
    else bySpace.set(spaceId, [plan])
  }
  // 每个空间最多 16 个主题扇区；零散的按类型归拢，颜色也就连成一片。
  const scattered = new Map<string, number[]>()
  const perSpaceTopics = new Map<string, number>()
  const sorted = [...clusters.entries()].sort(
    (left, right) => right[1].length - left[1].length || left[0].localeCompare(right[0]),
  )
  for (const [key, members] of sorted) {
    const [spaceId, cluster] = key.split('\u0000')
    const used = perSpaceTopics.get(spaceId) || 0
    if (cluster && members.length >= threshold && used < 16) {
      perSpaceTopics.set(spaceId, used + 1)
      addTo(spaceId, { key, label: cluster, kind: 'topic', spaceId, members })
      continue
    }
    for (const index of members) {
      const typeKey = `${spaceId}\u0000type:${facts[index].type}`
      const list = scattered.get(typeKey)
      if (list) list.push(index)
      else scattered.set(typeKey, [index])
    }
  }
  for (const [key, members] of scattered) {
    const [spaceId, cluster] = key.split('\u0000')
    addTo(spaceId, { key, label: cluster.slice(5), kind: 'type', spaceId, members })
  }
  return bySpace
}

function layoutSectors(
  facts: readonly MemoryEvolutionFact[],
  appears: Float64Array,
  spaces: readonly MemorySpace[],
) {
  const bySpace = planSectors(facts)
  const spaceIds = [...bySpace.keys()].sort((left, right) => {
    if (left === 'global') return -1
    if (right === 'global') return 1
    return stableHash(left) - stableHash(right)
  })
  const sectors: EvolutionSector[] = []
  const sectorOf = new Int16Array(facts.length)
  const theta = new Float32Array(facts.length)
  const spaceArcs: EvolutionSpaceArc[] = []
  const spaceGap = spaceIds.length > 1 ? 0.09 : 0
  const totalSectors = spaceIds.reduce((sum, id) => sum + (bySpace.get(id)?.length || 0), 0)
  const sectorGap = Math.min(0.022, (TAU * 0.25) / Math.max(1, totalSectors))
  const spaceWeight = (id: string) =>
    (bySpace.get(id) || []).reduce((sum, plan) => sum + Math.sqrt(plan.members.length) + 0.6, 0)
  const totalWeight = spaceIds.reduce((sum, id) => sum + spaceWeight(id), 0) || 1
  const usable = TAU - spaceGap * spaceIds.length - sectorGap * totalSectors
  let cursor = -Math.PI / 2 + spaceGap / 2
  for (const spaceId of spaceIds) {
    const plans = [...(bySpace.get(spaceId) || [])].sort(
      (left, right) => stableHash(left.key) - stableHash(right.key),
    )
    const spaceStart = cursor
    for (const plan of plans) {
      const width = (usable * (Math.sqrt(plan.members.length) + 0.6)) / totalWeight
      const sectorIndex = sectors.length
      sectors.push({
        key: plan.key.split('\u0000')[1],
        label: plan.label,
        kind: plan.kind,
        spaceId,
        start: cursor,
        end: cursor + width,
        count: plan.members.length,
      })
      // 按时间顺序取黄金分割：均匀铺开；新事实排在后面，旧事实的角度不会被挤动。
      const ordered = [...plan.members].sort(
        (left, right) =>
          appears[left] - appears[right] || facts[left].id.localeCompare(facts[right].id),
      )
      const seed = hash01(plan.key)
      ordered.forEach((index, order) => {
        sectorOf[index] = sectorIndex
        const fraction = (seed + order * 0.61803398875) % 1
        theta[index] = cursor + width * (0.05 + 0.9 * fraction)
      })
      cursor += width + sectorGap
    }
    spaceArcs.push({
      spaceId,
      name: spaces.find((space) => space.id === spaceId)?.name || spaceId,
      start: spaceStart,
      end: cursor - sectorGap,
    })
    cursor += spaceGap
  }
  return { sectors, sectorOf, theta, spaceArcs: spaceIds.length > 1 ? spaceArcs : [] }
}

function sortedCopy(values: number[]) {
  return Float64Array.from(values).sort()
}

export function buildEvolutionScene(
  data: MemoryEvolutionData,
  { histogramBins = 160 }: { histogramBins?: number } = {},
): EvolutionScene {
  const now = parseTime(data.now, Date.now())
  // 按 id 排序：布局不受接口返回顺序影响。
  const facts = [...data.facts].sort((left, right) => left.id.localeCompare(right.id))
  const count = facts.length
  const domain = computeDomain(facts, now)
  const span = domain.end - domain.start
  const lateThreshold = Math.max(DAY_MS, span * 0.015)
  const indexById = new Map(facts.map((fact, index) => [fact.id, index]))

  const validAt = new Float64Array(count)
  const appearAt = new Float64Array(count)
  const replacedAt = new Float64Array(count).fill(Infinity)
  const archivedAt = new Float64Array(count).fill(Infinity)
  const successor = new Int32Array(count).fill(-1)
  const flags = new Uint8Array(count)
  const typeIndex = new Uint8Array(count)
  const predecessors = new Map<number, number[]>()

  facts.forEach((fact, index) => {
    const { created, valid, appear } = parseFactTimes(fact, now)
    validAt[index] = valid
    appearAt[index] = appear
    const type = EVOLUTION_TYPES.indexOf(fact.type)
    typeIndex[index] = type >= 0 ? type : EVOLUTION_TYPES.indexOf('concept')
    let flag = 0
    if (fact.pinned) flag |= FLAG_PINNED
    if (fact.tainted) flag |= FLAG_TAINTED
    if (fact.status === 'stale') flag |= FLAG_STALE
    if (created - valid > lateThreshold) flag |= FLAG_LATE
    flags[index] = flag
    const next = fact.supersededBy ? indexById.get(fact.supersededBy) : undefined
    if (next !== undefined && next !== index) {
      successor[index] = next
      const list = predecessors.get(next)
      if (list) list.push(index)
      else predecessors.set(next, [index])
    }
  })

  // 被替换、归档的时刻都不早于它出现：导入的旧数据可能时间颠倒。
  facts.forEach((fact, index) => {
    const replaced =
      parseTime(fact.supersededAt, NaN) ||
      parseTime(fact.validTo, NaN) ||
      (fact.status === 'superseded'
        ? successor[index] >= 0
          ? appearAt[successor[index]]
          : domain.end
        : NaN)
    if (Number.isFinite(replaced)) replacedAt[index] = Math.max(replaced, appearAt[index])
    const archived =
      parseTime(fact.archivedAt, NaN) || (fact.status === 'archived' ? domain.end : NaN)
    if (Number.isFinite(archived)) archivedAt[index] = Math.max(archived, appearAt[index])
  })

  const { sectors, sectorOf, theta, spaceArcs } = layoutSectors(facts, appearAt, data.spaces)
  const rho = new Float32Array(count)
  const x = new Float32Array(count)
  const y = new Float32Array(count)
  const lift = new Float32Array(count)
  const size = new Float32Array(count)
  const brightness = new Float32Array(count)
  const phase = new Float32Array(count)
  facts.forEach((fact, index) => {
    rho[index] = rhoAt(domain, validAt[index])
    x[index] = discX(theta[index], rho[index])
    y[index] = discY(theta[index], rho[index])
    const importance = clamp01(Number(fact.importance) || 0)
    const authority = clamp01((Number(fact.authority) || 0) / 100)
    // 主次要拉开：大多数记忆是细小的光点，重要的才亮得显眼。
    size[index] = 0.16 + 0.84 * importance ** 1.5
    brightness[index] = (0.4 + 0.22 * authority + 0.38 * importance) * (fact.tainted ? 0.6 : 1)
    lift[index] = 0.016 + 0.105 * importance ** 1.5 + (fact.pinned ? 0.03 : 0)
    phase[index] = hash01(fact.id) * TAU
  })

  const replacements: EvolutionReplacement[] = []
  facts.forEach((_, index) => {
    if (Number.isFinite(replacedAt[index]))
      replacements.push({ at: replacedAt[index], from: index, to: successor[index] })
  })
  replacements.sort((left, right) => left.at - right.at || left.from - right.from)

  const bins = Math.max(1, histogramBins)
  const histogram = new Uint16Array(bins)
  const histogramReplacements = new Uint16Array(bins)
  const binOf = (at: number) =>
    Math.min(bins - 1, Math.floor(clamp01((at - domain.start) / span) * bins))
  for (let index = 0; index < count; index += 1) {
    if (appearAt[index] <= domain.end) histogram[binOf(appearAt[index])] += 1
  }
  for (const item of replacements) {
    if (item.at <= domain.end) histogramReplacements[binOf(item.at)] += 1
  }

  const appears = [...appearAt]
  const ended: number[] = []
  const archived: number[] = []
  for (let index = 0; index < count; index += 1) {
    const end = Math.min(replacedAt[index], archivedAt[index])
    if (Number.isFinite(end)) ended.push(end)
    if (Number.isFinite(archivedAt[index])) archived.push(archivedAt[index])
  }

  const digests = data.digests.map((digest): EvolutionDigestArc => {
    const at = parseTime(digest.windowEnd, domain.end)
    const start = parseTime(digest.windowStart, at)
    const arc = spaceArcs.find((item) => item.spaceId === digest.spaceId)
    const base = arc?.start ?? -Math.PI
    const range = (arc?.end ?? Math.PI) - base
    const lengthByLevel = digest.level === 'month' ? 0.16 : digest.level === 'week' ? 0.085 : 0.045
    const length = range * lengthByLevel
    const offset = base + hash01(digest.id) * Math.max(0, range - length)
    return {
      id: digest.id,
      level: digest.level,
      at,
      rho: rhoAt(domain, (start + at) / 2),
      start: offset,
      end: offset + length,
    }
  })

  const unit = chooseRingUnit(domain)
  return {
    facts,
    count,
    indexById,
    domain,
    unit,
    rings: buildRings(domain, unit, appears),
    bornWindow: Math.max(span * 0.012, 1),
    validAt,
    appearAt,
    replacedAt,
    archivedAt,
    theta,
    rho,
    x,
    y,
    lift,
    size,
    brightness,
    phase,
    typeIndex,
    sector: sectorOf,
    flags,
    successor,
    predecessors,
    replacements,
    sectors,
    spaceArcs,
    digests,
    histogram,
    histogramReplacements,
    truncated: Boolean(data.truncated),
    sortedAppear: sortedCopy(appears),
    sortedEnded: sortedCopy(ended),
    sortedArchived: sortedCopy(archived),
  }
}

export function factStateAt(scene: EvolutionScene, index: number, at: number): FactStateCode {
  if (at < scene.appearAt[index]) return FACT_HIDDEN
  if (at >= scene.archivedAt[index]) return FACT_ARCHIVED
  if (at >= scene.replacedAt[index]) return FACT_SUPERSEDED
  return FACT_ACTIVE
}

export function factStateNameAt(
  scene: EvolutionScene,
  index: number,
  at: number,
  bornWindow = scene.bornWindow,
): FactStateName {
  const state = factStateAt(scene, index, at)
  if (state === FACT_HIDDEN) return 'hidden'
  if (state === FACT_ARCHIVED) return 'archived'
  if (state === FACT_SUPERSEDED) return 'superseded'
  return at - scene.appearAt[index] < bornWindow ? 'born' : 'active'
}

function countUpTo(sorted: Float64Array, at: number) {
  let low = 0
  let high = sorted.length
  while (low < high) {
    const middle = (low + high) >> 1
    if (sorted[middle] <= at) low = middle + 1
    else high = middle
  }
  return low
}

// 读数用二分：拖动时每帧都要算，不遍历全部事实。
export function statsAt(scene: EvolutionScene, at: number): EvolutionStats {
  const known = countUpTo(scene.sortedAppear, at)
  const ended = countUpTo(scene.sortedEnded, at)
  const archived = countUpTo(scene.sortedArchived, at)
  return { active: known - ended, superseded: ended - archived, archived, known }
}

// 一条事实的完整版本链：从最早到最新。
export function versionChainOf(scene: EvolutionScene, index: number) {
  const seen = new Set<number>([index])
  const older: number[] = []
  const frontier = [index]
  while (frontier.length) {
    for (const previous of scene.predecessors.get(frontier.pop() as number) || []) {
      if (seen.has(previous)) continue
      seen.add(previous)
      older.push(previous)
      frontier.push(previous)
    }
  }
  const newer: number[] = []
  let cursor = scene.successor[index]
  while (cursor >= 0 && !seen.has(cursor)) {
    seen.add(cursor)
    newer.push(cursor)
    cursor = scene.successor[cursor]
  }
  return [...older, index, ...newer].sort(
    (left, right) => scene.appearAt[left] - scene.appearAt[right] || left - right,
  )
}

export function clampEvolutionZoom(zoom: number) {
  return Math.min(EVOLUTION_MAX_ZOOM, Math.max(EVOLUTION_MIN_ZOOM, zoom))
}

export function defaultEvolutionZoom(stageWidth: number, factCount: number) {
  return stageWidth < 720 && factCount >= NARROW_ZOOM_MIN_FACTS ? EVOLUTION_NARROW_ZOOM : 1
}

// 放大时取景往近处的外圈靠：1 倍看整个圆盘，1.5 倍起近侧的外圈固定在取景下方，
// 放得再大，最新的一圈也不会跑出画面。
export function evolutionRimFocus(zoom: number) {
  const progress = clamp01((zoom - 1) / 0.5)
  return progress * progress * (3 - 2 * progress)
}

// 选中时要看版本链和卡片：退回看整个圆盘，只在用户自己放大过时保留多出来的那部分。
export function selectedEvolutionZoom(zoom: number, defaultZoom: number) {
  return Math.max(1, zoom / defaultZoom) * EVOLUTION_SELECTED_ZOOM
}

// 视觉重心放在现在：离生长前沿越远越暗。按当前时刻算，拖动时间轴时亮的永远是那一刻最新的一圈。
export function evolutionAgeFade(rho: number, frontierRho: number) {
  const progress = clamp01(rho / Math.max(frontierRho, 0.001))
  return OLDEST_FADE + (1 - OLDEST_FADE) * progress * progress * (3 - 2 * progress)
}

// 捏合时缩放倍数跟两指间距的比例走，手指张开一倍画面就放大一倍。
export function pinchEvolutionZoom(startZoom: number, startSpread: number, spread: number) {
  return clampEvolutionZoom((startZoom * spread) / Math.max(1, startSpread))
}

export function timeFromSlider(domain: EvolutionDomain, value: number) {
  return domain.start + (domain.end - domain.start) * clamp01(value / SLIDER_MAX)
}

export function sliderFromTime(domain: EvolutionDomain, at: number) {
  const span = Math.max(1, domain.end - domain.start)
  return Math.round(clamp01((at - domain.start) / span) * SLIDER_MAX)
}

// 开场的时间曲线：两头慢、中间快，第一批记忆看得清，落到现在时稳稳停住。
export function openingProgress(elapsedMs: number, durationMs = OPENING_DURATION_MS) {
  const u = clamp01(elapsedMs / durationMs)
  return u < 0.5 ? 4 * u * u * u : 1 - (-2 * u + 2) ** 3 / 2
}
