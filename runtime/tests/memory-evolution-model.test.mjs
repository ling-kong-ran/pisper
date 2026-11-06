import assert from 'node:assert/strict'
import test from 'node:test'
import {
  buildEvolutionScene,
  buildRings,
  chooseRingUnit,
  clampEvolutionZoom,
  computeDomain,
  discX,
  discY,
  FACT_ACTIVE,
  FACT_ARCHIVED,
  FACT_HIDDEN,
  FACT_SUPERSEDED,
  factStateAt,
  factStateNameAt,
  FLAG_LATE,
  FLAG_PINNED,
  EVOLUTION_MAX_ZOOM,
  EVOLUTION_MIN_ZOOM,
  openingProgress,
  pinchEvolutionZoom,
  rhoAt,
  sliderFromTime,
  SLIDER_MAX,
  statsAt,
  timeFromSlider,
  topicClusterOf,
  versionChainOf,
} from '../../src/features/memory/memory-evolution-model.ts'

const DAY = 86_400_000
const NOW = Date.UTC(2026, 10, 5, 12)
const iso = (daysAgo) => new Date(NOW - daysAgo * DAY).toISOString()

function fact(id, daysAgo, overrides = {}) {
  return {
    id,
    spaceId: 'global',
    title: `记忆 ${id}`,
    type: 'fact',
    topicKey: '',
    importance: 0.5,
    authority: 90,
    status: 'active',
    sourceType: 'conversation',
    validFrom: iso(daysAgo),
    validTo: '',
    createdAt: iso(daysAgo),
    supersededBy: '',
    supersededAt: '',
    archivedAt: '',
    archiveReason: '',
    pinned: false,
    tainted: false,
    confirmCount: 1,
    lastConfirmedAt: iso(daysAgo),
    accessCount: 0,
    ...overrides,
  }
}

function scene(facts, overrides = {}) {
  return buildEvolutionScene({
    spaceId: 'all',
    now: new Date(NOW).toISOString(),
    spaces: [{ id: 'global', name: '全局', kind: 'global', nodeCount: facts.length }],
    facts,
    digests: [],
    truncated: false,
    ...overrides,
  })
}

function indexOf(built, id) {
  return built.indexById.get(id)
}

test('the time domain starts before the first fact and ends at now', () => {
  const domain = computeDomain([fact('a', 100), fact('b', 10)], NOW)
  assert.ok(domain.start < NOW - 100 * DAY)
  assert.equal(domain.end, NOW)
  // 回填的事实：valid_from 更早时，域要覆盖到它落的那一圈。
  const backdated = computeDomain([fact('a', 10, { validFrom: iso(200) })], NOW)
  assert.ok(backdated.start < NOW - 200 * DAY)
})

test('an empty or very short history still has a domain with rings', () => {
  const empty = computeDomain([], NOW)
  assert.equal(empty.end, NOW)
  assert.ok(NOW - empty.start >= 150 * DAY)
  const short = computeDomain([fact('a', 0.1)], NOW)
  assert.ok(NOW - short.start >= 7 * DAY)
  assert.equal(chooseRingUnit(short), 'day')
  assert.ok(scene([fact('a', 1), fact('b', 0.5)]).rings.length >= 6)
  assert.ok(scene([]).rings.length >= 4)
})

test('the ring unit follows the span', () => {
  const at = (days) => ({ start: NOW - days * DAY, end: NOW })
  assert.equal(chooseRingUnit(at(14)), 'day')
  assert.equal(chooseRingUnit(at(90)), 'week')
  assert.equal(chooseRingUnit(at(420)), 'month')
  assert.equal(chooseRingUnit(at(2000)), 'quarter')
  assert.equal(chooseRingUnit(at(6000)), 'year')
})

test('rings grow outward in time and record how busy each interval was', () => {
  const domain = { start: NOW - 400 * DAY, end: NOW }
  const appears = [NOW - 300 * DAY, NOW - 301 * DAY, NOW - 302 * DAY, NOW - 100 * DAY]
  const rings = buildRings(domain, 'month', appears)
  assert.ok(rings.length >= 12)
  for (let index = 1; index < rings.length; index += 1) {
    assert.ok(rings[index].rho > rings[index - 1].rho)
    assert.ok(rings[index].at > rings[index - 1].at)
  }
  assert.ok(rings.every((ring) => ring.rho > 0 && ring.rho <= 1))
  assert.equal(Math.max(...rings.map((ring) => ring.activity)), 1)
  assert.ok(rings.some((ring) => ring.major))
})

test('time maps to radius monotonically, from the core to the rim', () => {
  const domain = { start: NOW - 100 * DAY, end: NOW }
  let previous = 0
  for (let index = 0; index <= 100; index += 1) {
    const rho = rhoAt(domain, domain.start + index * DAY)
    assert.ok(rho > previous)
    previous = rho
  }
  assert.equal(rhoAt(domain, domain.end), 1)
  assert.ok(rhoAt(domain, domain.start) > 0)
  assert.equal(rhoAt(domain, domain.end + DAY), 1)
})

test('states change exactly at the appear, replace and archive moments', () => {
  const built = scene([
    fact('old', 100, {
      status: 'superseded',
      supersededBy: 'new',
      supersededAt: iso(50),
      validTo: iso(50),
    }),
    fact('new', 50),
    fact('gone', 80, { status: 'archived', archivedAt: iso(20), archiveReason: 'expired' }),
  ])
  const old = indexOf(built, 'old')
  const gone = indexOf(built, 'gone')
  const appear = built.appearAt[old]
  const replaced = built.replacedAt[old]
  assert.equal(factStateAt(built, old, appear - 1), FACT_HIDDEN)
  assert.equal(factStateAt(built, old, appear), FACT_ACTIVE)
  assert.equal(factStateAt(built, old, replaced - 1), FACT_ACTIVE)
  assert.equal(factStateAt(built, old, replaced), FACT_SUPERSEDED)
  assert.equal(factStateAt(built, gone, built.archivedAt[gone] - 1), FACT_ACTIVE)
  assert.equal(factStateAt(built, gone, built.archivedAt[gone]), FACT_ARCHIVED)
  assert.equal(factStateNameAt(built, old, appear), 'born')
  assert.equal(factStateNameAt(built, old, appear + built.bornWindow), 'active')
  assert.equal(factStateNameAt(built, old, appear - 1), 'hidden')
  assert.equal(factStateNameAt(built, old, replaced), 'superseded')
  assert.equal(factStateNameAt(built, gone, built.archivedAt[gone]), 'archived')
})

test('a replaced fact that is later archived ends up archived', () => {
  const built = scene([
    fact('a', 100, {
      status: 'archived',
      supersededAt: iso(60),
      validTo: iso(60),
      archivedAt: iso(30),
    }),
  ])
  assert.equal(factStateAt(built, 0, NOW - 40 * DAY), FACT_SUPERSEDED)
  assert.equal(factStateAt(built, 0, NOW - 10 * DAY), FACT_ARCHIVED)
})

test('something learned late appears when it was learned, on the ring when it happened', () => {
  const built = scene([
    fact('late', 10, { validFrom: iso(90), createdAt: iso(10) }),
    fact('b', 200),
  ])
  const late = indexOf(built, 'late')
  assert.ok(built.flags[late] & FLAG_LATE)
  assert.equal(built.appearAt[late], NOW - 10 * DAY)
  assert.ok(Math.abs(built.rho[late] - rhoAt(built.domain, NOW - 90 * DAY)) < 1e-6)
  assert.equal(factStateAt(built, late, NOW - 50 * DAY), FACT_HIDDEN)
  assert.equal(factStateAt(built, late, NOW - 5 * DAY), FACT_ACTIVE)
  // 几小时的差距不算“晚知道”。
  const soon = scene([
    fact('s', 10, { createdAt: new Date(NOW - 10 * DAY + 3_600_000).toISOString() }),
  ])
  assert.equal(soon.flags[0] & FLAG_LATE, 0)
})

test('timestamps that run backwards never end a fact before it appears', () => {
  const built = scene([
    fact('a', 10, { status: 'superseded', supersededAt: iso(50) }),
    fact('b', 10, { status: 'archived', archivedAt: iso(50) }),
    fact('c', 10, { status: 'superseded' }),
    fact('d', 10, { status: 'archived' }),
  ])
  for (let index = 0; index < built.count; index += 1) {
    assert.ok(Math.min(built.replacedAt[index], built.archivedAt[index]) >= built.appearAt[index])
  }
  assert.equal(factStateAt(built, indexOf(built, 'c'), NOW), FACT_SUPERSEDED)
  assert.equal(factStateAt(built, indexOf(built, 'd'), NOW), FACT_ARCHIVED)
})

test('layout is deterministic, ignores input order and does not depend on the moment', () => {
  const facts = Array.from({ length: 60 }, (_, index) =>
    fact(`f${index}`, (index * 7) % 300, { topicKey: `project.topic${index % 5}.item${index}` }),
  )
  const first = scene(facts)
  const second = scene([...facts].reverse())
  for (let index = 0; index < first.count; index += 1) {
    assert.equal(first.facts[index].id, second.facts[index].id)
    assert.equal(first.x[index], second.x[index])
    assert.equal(first.y[index], second.y[index])
    assert.equal(first.theta[index], second.theta[index])
  }
  const before = [...first.x]
  for (const moment of [first.domain.start, NOW - 100 * DAY, NOW]) {
    statsAt(first, moment)
    for (let index = 0; index < first.count; index += 1) factStateAt(first, index, moment)
  }
  assert.deepEqual([...first.x], before)
  // 位置就是由角度和半径算出来的，不是另外随机的。
  assert.ok(Math.abs(first.x[0] - discX(first.theta[0], first.rho[0])) < 1e-5)
  assert.ok(Math.abs(first.y[0] - discY(first.theta[0], first.rho[0])) < 1e-5)
})

test('adding a newer fact does not move older facts of the same topic within their sector', () => {
  const facts = Array.from({ length: 10 }, (_, index) =>
    fact(`f${index}`, 100 - index, { topicKey: 'project.database.engine' }),
  )
  const before = scene(facts)
  const after = scene([...facts, fact('later', 1, { topicKey: 'project.database.pool' })])
  const span = (built) => {
    const sector = built.sectors[0]
    return (built.theta[indexOf(built, 'f3')] - sector.start) / (sector.end - sector.start)
  }
  assert.ok(Math.abs(span(before) - span(after)) < 1e-5)
})

test('topic clusters skip generic segments', () => {
  assert.equal(topicClusterOf('project.database.engine'), 'database')
  assert.equal(topicClusterOf('user.language'), 'language')
  assert.equal(topicClusterOf('ops.step.3'), 'ops')
  assert.equal(topicClusterOf('project'), 'project')
  assert.equal(topicClusterOf(''), '')
})

test('facts of one topic sit next to each other, and scattered ones group by type', () => {
  const facts = [
    ...Array.from({ length: 12 }, (_, index) =>
      fact(`db${index}`, 20 + index, { topicKey: 'project.database.engine' }),
    ),
    ...Array.from({ length: 8 }, (_, index) =>
      fact(`ui${index}`, 40 + index, { topicKey: 'project.ui.theme' }),
    ),
    ...Array.from({ length: 5 }, (_, index) =>
      fact(`loner${index}`, 60 + index, { topicKey: `solo${index}`, type: 'risk' }),
    ),
  ]
  const built = scene(facts)
  const sectorOf = (id) => built.sectors[built.sector[indexOf(built, id)]]
  assert.equal(sectorOf('db0').label, 'database')
  assert.equal(sectorOf('db0'), sectorOf('db11'))
  assert.equal(sectorOf('ui0').label, 'ui')
  assert.equal(sectorOf('loner0').kind, 'type')
  assert.equal(sectorOf('loner0'), sectorOf('loner4'))
  for (const item of facts) {
    const sector = sectorOf(item.id)
    const angle = built.theta[indexOf(built, item.id)]
    assert.ok(angle >= sector.start && angle <= sector.end)
  }
  // 扇区互不重叠，合起来不超过一整圈。
  const sorted = [...built.sectors].sort((left, right) => left.start - right.start)
  for (let index = 1; index < sorted.length; index += 1)
    assert.ok(sorted[index].start >= sorted[index - 1].end)
  assert.ok(sorted.at(-1).end - sorted[0].start <= Math.PI * 2)
})

test('several spaces each get their own arc; one space does not need any', () => {
  const single = scene([fact('a', 10), fact('b', 20)])
  assert.deepEqual(single.spaceArcs, [])
  const built = scene(
    [
      fact('a', 10),
      fact('b', 20, { spaceId: 'project-x' }),
      fact('c', 30, { spaceId: 'project-x' }),
    ],
    {
      spaces: [
        { id: 'global', name: '全局', kind: 'global', nodeCount: 1 },
        { id: 'project-x', name: 'acme-web', kind: 'project', nodeCount: 2 },
      ],
    },
  )
  assert.equal(built.spaceArcs.length, 2)
  assert.equal(built.spaceArcs[0].spaceId, 'global')
  assert.equal(built.spaceArcs[1].name, 'acme-web')
  assert.ok(built.spaceArcs[1].start >= built.spaceArcs[0].end)
})

test('the histogram counts every fact once and marks replacements', () => {
  const facts = Array.from({ length: 50 }, (_, index) => fact(`f${index}`, index * 3 + 1))
  facts.push(
    fact('old', 90, {
      status: 'superseded',
      supersededBy: 'f0',
      supersededAt: iso(1),
      validTo: iso(1),
    }),
  )
  const built = scene(facts)
  assert.equal(built.histogram.length, 160)
  assert.equal(
    built.histogram.reduce((sum, value) => sum + value, 0),
    51,
  )
  assert.equal(
    built.histogramReplacements.reduce((sum, value) => sum + value, 0),
    1,
  )
  assert.equal(built.histogram[0], 0)
})

test('replacements are sorted by time and point from the old fact to the new one', () => {
  const built = scene([
    fact('a1', 100, { status: 'superseded', supersededBy: 'a2', supersededAt: iso(60) }),
    fact('a2', 60, { status: 'superseded', supersededBy: 'a3', supersededAt: iso(10) }),
    fact('a3', 10),
    fact('b1', 80, { validTo: iso(70) }),
  ])
  assert.deepEqual(
    built.replacements.map((item) => built.facts[item.from].id),
    ['b1', 'a1', 'a2'],
  )
  const first = built.replacements[1]
  assert.equal(built.facts[first.to].id, 'a2')
  assert.equal(built.replacements[0].to, -1)
  for (let index = 1; index < built.replacements.length; index += 1)
    assert.ok(built.replacements[index].at >= built.replacements[index - 1].at)
})

test('a version chain is found from any version, oldest first', () => {
  const built = scene([
    fact('a1', 100, { status: 'superseded', supersededBy: 'a2', supersededAt: iso(60) }),
    fact('a2', 60, { status: 'superseded', supersededBy: 'a3', supersededAt: iso(10) }),
    fact('a3', 10),
    fact('other', 40),
  ])
  for (const id of ['a1', 'a2', 'a3']) {
    assert.deepEqual(
      versionChainOf(built, indexOf(built, id)).map((index) => built.facts[index].id),
      ['a1', 'a2', 'a3'],
    )
  }
  assert.deepEqual(versionChainOf(built, indexOf(built, 'other')), [indexOf(built, 'other')])
})

test('a cycle in supersededBy does not hang', () => {
  const built = scene([
    fact('a', 50, { status: 'superseded', supersededBy: 'b', supersededAt: iso(40) }),
    fact('b', 40, { status: 'superseded', supersededBy: 'a', supersededAt: iso(30) }),
  ])
  assert.equal(versionChainOf(built, 0).length, 2)
})

test('the readout at any moment matches counting every fact by hand', () => {
  const facts = Array.from({ length: 120 }, (_, index) => {
    const start = 10 + index * 3
    if (index % 7 === 0)
      return fact(`f${index}`, start, {
        status: 'superseded',
        supersededAt: iso(start - 5),
        validTo: iso(start - 5),
      })
    if (index % 11 === 0)
      return fact(`f${index}`, start, { status: 'archived', archivedAt: iso(start - 4) })
    return fact(`f${index}`, start)
  })
  const built = scene(facts)
  for (let step = 0; step <= 50; step += 1) {
    const at = built.domain.start + ((built.domain.end - built.domain.start) * step) / 50
    const expected = { active: 0, superseded: 0, archived: 0, known: 0 }
    for (let index = 0; index < built.count; index += 1) {
      const state = factStateAt(built, index, at)
      if (state !== FACT_HIDDEN) expected.known += 1
      if (state === FACT_ACTIVE) expected.active += 1
      if (state === FACT_SUPERSEDED) expected.superseded += 1
      if (state === FACT_ARCHIVED) expected.archived += 1
    }
    assert.deepEqual(statsAt(built, at), expected)
  }
})

test('pinned facts and important facts stand out', () => {
  const built = scene([
    fact('quiet', 10, { importance: 0.1, authority: 40 }),
    fact('loud', 12, { importance: 1, authority: 100, pinned: true }),
    fact('untrusted', 14, { importance: 1, authority: 100, tainted: true }),
  ])
  const quiet = indexOf(built, 'quiet')
  const loud = indexOf(built, 'loud')
  assert.ok(built.size[loud] > built.size[quiet])
  assert.ok(built.brightness[loud] > built.brightness[quiet])
  assert.ok(built.lift[loud] > built.lift[quiet])
  assert.ok(built.flags[loud] & FLAG_PINNED)
  assert.ok(built.brightness[indexOf(built, 'untrusted')] < built.brightness[loud])
})

test('digests become arcs that appear when their window closes', () => {
  const built = scene([fact('a', 100), fact('b', 5)], {
    digests: [
      {
        id: 'd1',
        spaceId: 'global',
        level: 'week',
        windowStart: iso(50),
        windowEnd: iso(43),
      },
    ],
  })
  const [arc] = built.digests
  assert.equal(arc.at, NOW - 43 * DAY)
  assert.ok(arc.end > arc.start)
  assert.ok(arc.rho > rhoAt(built.domain, NOW - 50 * DAY) - 1e-9)
  assert.ok(arc.rho < rhoAt(built.domain, NOW - 43 * DAY) + 1e-9)
})

test('unknown types and broken dates do not break the scene', () => {
  const built = scene([
    fact('a', 10, { type: 'mystery', validFrom: 'not-a-date', createdAt: '' }),
    fact('b', 20, { importance: Number.NaN, authority: undefined }),
  ])
  assert.equal(built.count, 2)
  for (let index = 0; index < built.count; index += 1) {
    assert.ok(Number.isFinite(built.x[index]))
    assert.ok(Number.isFinite(built.size[index]))
    assert.ok(Number.isFinite(built.appearAt[index]))
  }
})

test('the timeline slider maps to time and back', () => {
  const domain = { start: NOW - 1000 * DAY, end: NOW }
  assert.equal(timeFromSlider(domain, 0), domain.start)
  assert.equal(timeFromSlider(domain, SLIDER_MAX), domain.end)
  assert.equal(sliderFromTime(domain, domain.end), SLIDER_MAX)
  assert.equal(sliderFromTime(domain, domain.start), 0)
  assert.equal(sliderFromTime(domain, timeFromSlider(domain, 437)), 437)
  assert.equal(sliderFromTime(domain, domain.end + DAY), SLIDER_MAX)
})

test('the opening curve starts slow, ends exactly at one and never goes backwards', () => {
  assert.equal(openingProgress(0), 0)
  assert.equal(openingProgress(6000), 1)
  assert.equal(openingProgress(9000), 1)
  assert.ok(openingProgress(600) < 0.01)
  let previous = 0
  for (let elapsed = 0; elapsed <= 6000; elapsed += 100) {
    const value = openingProgress(elapsed)
    assert.ok(value >= previous)
    previous = value
  }
})

test('pinching scales with the finger spread and stays within the zoom range', () => {
  assert.equal(pinchEvolutionZoom(1, 100, 200), 2)
  assert.equal(pinchEvolutionZoom(1.5, 200, 100), 0.75)
  assert.equal(pinchEvolutionZoom(1, 100, 1000), EVOLUTION_MAX_ZOOM)
  assert.equal(pinchEvolutionZoom(1, 100, 1), EVOLUTION_MIN_ZOOM)
  // 两指落在同一点时不能除以零。
  assert.equal(pinchEvolutionZoom(1, 0, 2), 2)
  assert.equal(clampEvolutionZoom(0.72), EVOLUTION_MIN_ZOOM)
  assert.equal(clampEvolutionZoom(9), EVOLUTION_MAX_ZOOM)
})
