import assert from 'node:assert/strict'
import { createHash } from 'node:crypto'
import { readFile } from 'node:fs/promises'
import test from 'node:test'

const root = new URL('../../', import.meta.url)
const read = (path) => readFile(new URL(path, root))
const raw = await read('shared/speech-resource-notices.json')
const notices = JSON.parse(raw.toString('utf8'))
const catalog = JSON.parse(await read('shared/speech-model-catalog.json'))
const catalogModels = new Map(catalog.models.map((model) => [model.id, model]))
const sha256 = (value) => createHash('sha256').update(value).digest('hex')
const shaPattern = /^[a-f0-9]{64}$/
const models = new Map(notices.models.map((model) => [model.id, model]))
const groups = new Map(
  notices.models.flatMap((model) => model.groups.map((group) => [group.id, group])),
)

// 锁定审计时读取的原文字节摘要，避免测试依赖被忽略的研究目录或在线来源。
const licenseSources = {
  'apache-2.0': {
    sha256: 'c71d239df91726fc519c6eb72d318ec65820627232b2f796219e87dcf35d0ab4',
    bytes: 11357,
    sourceUrl:
      'https://raw.githubusercontent.com/Gilgamesh-J/X-ASR/838297cd47fed858e6cf72eaf5a52f948a3edd73/LICENSE',
    copyright: 'Copyright [yyyy] [name of copyright owner]',
    ending: 'limitations under the License.\n',
  },
  'apache-2.0-sherpa': {
    sha256: 'cfc7749b96f63bd31c3c42b5c471bf756814053e847c10f3eb003417bc523d30',
    bytes: 11358,
    sourceUrl:
      'https://raw.githubusercontent.com/k2-fsa/sherpa-onnx/917bed95c8e5c7c18aa4d69fea42e9ef8ef0a60e/LICENSE',
    copyright: 'Copyright [yyyy] [name of copyright owner]',
    ending: 'limitations under the License.\n',
  },
  'mit-melo': {
    sha256: '88a50e5a02bbc2a5c2f084dc19da751aa97b1690f5fda76cd8005c8634d1ca70',
    bytes: 1053,
    sourceUrl:
      'https://raw.githubusercontent.com/myshell-ai/MeloTTS/209145371cff8fc3bd60d7be902ea69cbdb7965a/LICENSE',
    copyright: 'Copyright (c) 2024 MyShell.ai',
    ending: 'SOFTWARE.',
  },
  'cmudict-0.6-1998': {
    sha256: '08ee91f3c05c55160bdc599247ba53e5de8cc2597766fdb22b54a81cb24dd954',
    bytes: 2381,
    sourceUrl:
      'https://raw.githubusercontent.com/myshell-ai/MeloTTS/209145371cff8fc3bd60d7be902ea69cbdb7965a/melo/text/cmudict.rep',
    copyright: 'Copyright 1998',
    ending: '## for forthcoming releases.\n##\n',
  },
  'mit-cppjieba': {
    sha256: 'ba898a14f729ba5e9965da34e3eecd5edd3795f2cc5d7c923b815ba79bb851b0',
    bytes: 1066,
    sourceUrl:
      'https://raw.githubusercontent.com/csukuangfj/cppjieba/f03dd931a00330c279c8590275850fd1f6749a18/LICENSE',
    copyright: 'Copyright (c) 2013\n',
    ending: 'CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.\n',
  },
  'mit-pypinyin': {
    sha256: '1e6c90014b4912815c296ee64bb6f6280af47e6d4c5d80e86232dfc5defe764c',
    bytes: 1105,
    sourceUrl:
      'https://raw.githubusercontent.com/mozillazg/python-pinyin/8595294b1a97845e30f11ecfdb3caa4e61ac3988/LICENSE.txt',
    copyright: 'Copyright (c) 2016 mozillazg, 闲耘 <hotoo.cn@gmail.com>',
    ending: 'CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.\n',
  },
}

function text(value) {
  assert.equal(typeof value, 'string')
  assert.ok(value.trim().length > 0)
  assert.ok(value.isWellFormed())
}

function unique(values) {
  assert.equal(new Set(values).size, values.length, `Duplicate values: ${values}`)
}

function relativePath(value, pattern = false) {
  text(value)
  const path = pattern && value.endsWith('/**') ? value.slice(0, -3) : value
  assert.ok(path.split('/').every((part) => part && part !== '.' && part !== '..'))
  assert.doesNotMatch(path, /[\\:*?\p{Cc}]/u)
}

function publicUrl(value) {
  const url = new URL(value)
  assert.equal(url.protocol, 'https:')
  assert.equal(url.username, '')
  assert.equal(url.password, '')
  assert.equal(url.search, '')
  if (url.hostname === 'ghfast.top') {
    assert.equal(
      url.pathname,
      '/https://github.com/csukuangfj/cppjieba/releases/download/sherpa-onnx-2024-04-19/dict.tar.bz2',
    )
    return
  }
  assert.ok(
    ['raw.githubusercontent.com', 'huggingface.co', 'hf-mirror.com', 'github.com'].includes(
      url.hostname,
    ),
  )
}

function evidence(value) {
  publicUrl(value.url)
  assert.match(value.sha256, shaPattern)
}

function matches(path, pattern) {
  return pattern.endsWith('/**') ? path.startsWith(pattern.slice(0, -2)) : path === pattern
}

function artifact(value) {
  relativePath(value.path)
  assert.ok(Number.isSafeInteger(value.bytes) && value.bytes > 0)
  assert.match(value.sha256, shaPattern)
  if (value.url) publicUrl(value.url)
}

function catalogFile(modelId, path) {
  const result = catalog.models
    .find((model) => model.id === modelId)
    ?.files.find((file) => file.path === path)
  assert.ok(result, `Missing catalog file ${modelId}/${path}`)
  return result
}

function sameArtifact(actual, expected) {
  assert.equal(actual.sha256, expected.sha256)
  assert.equal(actual.bytes, expected.bytes)
}

test('offline speech notices have a bounded schema, unique IDs and valid references', () => {
  assert.equal(notices.schemaVersion, 1)
  assert.ok(raw.length < 128 * 1024)
  text(notices.scope)
  text(notices.licenseIdsMeaning)
  assert.deepEqual(Object.keys(notices.statusDefinitions).sort(), [
    'confirmed',
    'declared',
    'pending',
  ])
  assert.deepEqual(Object.keys(notices.pathScopes).sort(), ['model', 'shared'])
  for (const value of [
    ...Object.values(notices.statusDefinitions),
    ...Object.values(notices.pathScopes),
  ])
    text(value)
  assert.equal(notices.models.length, 2)
  assert.equal(notices.licenses.length, 6)
  const allIds = notices.licenses.map((license) => license.id)
  const licenseIds = new Set(allIds)
  for (const license of notices.licenses) {
    text(license.id)
    text(license.name)
    text(license.text)
    evidence({ url: license.sourceUrl, sha256: license.sha256 })
  }
  for (const model of notices.models) {
    allIds.push(model.id)
    text(model.revision)
    text(model.attribution)
    assert.ok(Array.isArray(model.groups) && model.groups.length > 0)
    assert.ok(Array.isArray(model.modifications))
    assert.equal(model.modifications.length, model.id === 'x-asr-480ms-int8' ? 1 : 0)
    const modelGroups = new Map(model.groups.map((group) => [group.id, group]))
    for (const group of model.groups) {
      allIds.push(group.id)
      assert.ok(Object.hasOwn(notices.pathScopes, group.pathScope))
      assert.ok(Object.hasOwn(notices.statusDefinitions, group.status))
      text(group.summary)
      assert.ok(Array.isArray(group.paths) && group.paths.length > 0)
      unique(group.paths)
      group.paths.forEach((path) => relativePath(path, true))
      assert.ok(Array.isArray(group.licenseIds) && group.licenseIds.length > 0)
      unique(group.licenseIds)
      for (const id of group.licenseIds) assert.ok(licenseIds.has(id), `Unknown license ${id}`)
      assert.ok(Array.isArray(group.evidence) && group.evidence.length > 0)
      unique(group.evidence.map((item) => item.url))
      group.evidence.forEach(evidence)
      assert.ok(Array.isArray(group.pendingReasons))
      assert.equal(group.pendingReasons.length > 0, group.status === 'pending')
      group.pendingReasons.forEach(text)
      for (const lead of group.sourceLeads ?? []) {
        publicUrl(lead.url)
        text(lead.summary)
        if (lead.sha256) assert.match(lead.sha256, shaPattern)
      }
    }
    for (const modification of model.modifications) {
      allIds.push(modification.id)
      assert.equal(modification.modifiedBy, 'Pisper')
      text(modification.summary)
      assert.ok(Array.isArray(modification.groupIds) && modification.groupIds.length > 0)
      unique(modification.groupIds)
      for (const id of modification.groupIds) {
        const group = modelGroups.get(id)
        assert.ok(group, `Unknown group ${id}`)
        assert.ok(['shared', 'derived'].includes(group.pathScope))
        assert.ok(group.paths.includes(modification.derived.path))
      }
      artifact(modification.source)
      artifact(modification.derived)
      relativePath(modification.generator.path)
      assert.match(modification.generator.sha256, shaPattern)
      text(modification.generator.method)
      for (const item of modification.localEvidence ?? []) {
        relativePath(item.path)
        assert.match(item.sha256, shaPattern)
      }
    }
  }
  allIds.forEach((id) => assert.match(id, /^[a-z0-9][a-z0-9._-]*$/))
  unique(allIds)
})

test('license texts retain complete audited bytes, source URLs and original copyrights', () => {
  assert.deepEqual(
    notices.licenses.map((license) => license.id).sort(),
    Object.keys(licenseSources).sort(),
  )
  for (const license of notices.licenses) {
    const expected = licenseSources[license.id]
    assert.equal(license.sourceUrl, expected.sourceUrl)
    assert.equal(license.sha256, expected.sha256)
    assert.equal(sha256(Buffer.from(license.text, 'utf8')), expected.sha256)
    assert.equal(Buffer.byteLength(license.text, 'utf8'), expected.bytes)
    assert.ok(license.text.includes(expected.copyright))
    assert.ok(license.text.endsWith(expected.ending))
  }
})

test('notice identities and explicit component scopes cover the catalog and BPE derivative', async () => {
  const identities = (items) =>
    items.map(({ id, revision }) => ({ id, revision })).sort((a, b) => a.id.localeCompare(b.id))
  assert.deepEqual(identities(notices.models), identities(catalog.models))
  for (const model of catalog.models) {
    const notice = models.get(model.id)
    const originalGroups = notice.groups.filter((group) => group.pathScope === 'model')
    for (const file of model.files) {
      const matched = originalGroups.filter((group) =>
        group.paths.some((path) => matches(file.path, path)),
      )
      if (model.id === 'vits-melo-tts-zh_en' && file.path === 'lexicon.txt') {
        // 同一文件混合两种独立来源的词典，必须同时保留两条许可链，而非任选其一。
        assert.deepEqual(matched.map((group) => group.id).sort(), [
          'melo-chinese-lexicon',
          'melo-english-lexicon',
        ])
      } else
        assert.equal(
          matched.length,
          1,
          `${model.id}/${file.path} must have exactly one component classification`,
        )
    }
    for (const group of originalGroups)
      for (const path of group.paths)
        assert.ok(
          model.files.some((file) => matches(file.path, path)),
          `Stale path ${path}`,
        )
    for (const modification of notice.modifications)
      sameArtifact(modification.source, catalogFile(model.id, modification.source.path))
    for (const group of notice.groups.filter((entry) => entry.pathScope !== 'model')) {
      for (const path of group.paths) {
        const bindings = notice.modifications.filter(
          (modification) =>
            modification.groupIds.includes(group.id) && modification.derived.path === path,
        )
        assert.equal(
          bindings.length,
          1,
          `Missing or ambiguous modification for ${group.id}/${path}`,
        )
        if (group.pathScope === 'shared') {
          const bytes = await read(`shared/${path}`)
          sameArtifact(bindings[0].derived, { bytes: bytes.length, sha256: sha256(bytes) })
        }
      }
    }
  }
  const asr = catalog.models.find((model) => model.id === 'x-asr-480ms-int8')
  assert.equal(
    models.get(asr.id).archive.url,
    asr.archive.urls.find((url) => url.startsWith('https://github.com/')),
  )
  assert.equal(models.get(asr.id).archive.sha256, asr.archive.sha256)
  const tts = catalog.models.find((model) => model.id === 'vits-melo-tts-zh_en')
  assert.equal(
    models.get(tts.id).archive.url,
    tts.archive.urls.find((url) => url.startsWith('https://github.com/')),
  )
  assert.equal(models.get(tts.id).archive.bytes, tts.archive.bytes)
  assert.equal(models.get(tts.id).archive.sha256, tts.archive.sha256)
  const fp32 = catalogModels.get('vits-melo-tts-zh_en')
  assert.deepEqual(models.get(fp32.id).archive, {
    url: fp32.archive.urls[1],
    bytes: fp32.archive.bytes,
    sha256: fp32.archive.sha256,
  })
  assert.deepEqual(groups.get('xasr-bpe-vocab').paths, [asr.config.bpeVocabResource])
})

test('unresolved provenance and distribution items remain explicit pending components', () => {
  const pending = {
    'xasr-weights': /checkpoint.*source commits/i,
    'xasr-tokens': /historical source revision/i,
    'xasr-bpe': /historical BPE source commit/i,
    'xasr-bpe-vocab': /source BPE historical provenance/i,
    'melo-weights': /Training-data permissions.*speaker consent/i,
    'melo-user-dictionary': /historical source commit.*license coverage/i,
    'melo-english-lexicon': /historical dictionary\/cache inputs/i,
    'melo-chinese-lexicon': /historical pypinyin versions/i,
    'melo-normalization-fsts': /generation rules.*source revisions/i,
  }
  for (const [id, reason] of Object.entries(pending)) {
    const group = groups.get(id)
    assert.ok(group, `Required pending component ${id} was removed`)
    assert.equal(group.status, 'pending')
    assert.match(group.pendingReasons.join('\n'), reason)
  }
  assert.deepEqual(groups.get('melo-cppjieba-dictionary').licenseIds, ['mit-cppjieba'])
  assert.deepEqual(groups.get('melo-english-lexicon').licenseIds, [
    'cmudict-0.6-1998',
    'mit-melo',
    'apache-2.0-sherpa',
  ])
  assert.deepEqual(groups.get('melo-chinese-lexicon').licenseIds, [
    'mit-pypinyin',
    'mit-melo',
    'apache-2.0-sherpa',
  ])
  assert.equal(groups.get('melo-cppjieba-dictionary').status, 'confirmed')
  assert.equal(groups.get('melo-package-notices').status, 'confirmed')
  assert.match(notices.scope, /separate license\/linkage and source-delivery audit/i)
  assert.match(notices.scope, /GPL components/i)
  assert.match(notices.licenseIdsMeaning, /not alternative dual-license choices/i)
  assert.match(notices.scope, /not legal advice, commercial-use clearance/i)
  assert.match(notices.scope, /does not package the large models/i)
})

test('BPE derivative retains its exact source, output and inspected generator hashes', async () => {
  const modification = models
    .get('x-asr-480ms-int8')
    .modifications.find((entry) => entry.id === 'xasr-bpe-extraction')
  assert.equal(
    modification.source.sha256,
    'f87a38025a5fdd1e4e9591f6a44bb81295097ce0b80df6f4ab9f44e52c64ca5f',
  )
  assert.equal(
    modification.derived.sha256,
    '01381aa0c3065832cb8d7462d529e3079a99be56c955ce93b4cb9b78e8aa34e5',
  )
  assert.equal(modification.generator.path, 'scripts/speech-bpe.mjs')
  assert.equal(modification.generator.sha256, sha256(await read(modification.generator.path)))
  assert.match(modification.generator.method, /sentencePieceModelToVocab/)
})

test('Melo retains the original single-speaker graph and has no local speaker derivatives', () => {
  const model = catalogModels.get('vits-melo-tts-zh_en')
  assert.equal(model.engine, 'vits')
  assert.deepEqual(
    model.voices.map(({ id, sid }) => ({ id, sid })),
    [{ id: 'melo-zh-en-female', sid: 0 }],
  )
  assert.equal(model.config.voiceSubset, undefined)
  assert.equal(model.config.voices, undefined)
  assert.deepEqual(models.get(model.id).modifications, [])
  assert.match(groups.get('melo-weights').summary, /n_speakers=1 and speaker_id=1/)
  assert.match(groups.get('melo-weights').summary, /maps public sid 0/)
  assert.match(groups.get('melo-weights').summary, /does not change the graph/)
})

test('unified catalog pins the approximately 160 MiB Melo archive and single default voice', () => {
  assert.equal(catalog.version, 1)
  assert.equal(catalog.models.length, 2)
  assert.deepEqual(catalog.defaults, {
    asr: 'x-asr-480ms-int8',
    tts: 'vits-melo-tts-zh_en',
    voice: 'melo-zh-en-female',
  })
  const fp32 = catalogModels.get(catalog.defaults.tts)
  assert.equal(fp32.name, 'MeloTTS Chinese + English FP32')
  assert.equal(fp32.revision, 'a0d5c6a264c0ef92d70d8661d8cc502d79627cd6')
  assert.deepEqual(fp32.config, {
    model: 'model.onnx',
    tokens: 'tokens.txt',
    lexicon: 'lexicon.txt',
    dictDir: 'dict',
    ruleFsts: ['date.fst', 'number.fst', 'phone.fst'],
    numThreads: 4,
    maxTextCodePoints: 16,
    noiseScale: 0.667,
    noiseScaleW: 0.8,
    lengthScale: 1,
  })
  assert.equal(fp32.files.length, 18)
  assert.equal(
    fp32.files.reduce((sum, file) => sum + file.bytes, 0),
    191224149,
  )
  sameArtifact(catalogFile(fp32.id, 'model.onnx'), {
    bytes: 170429550,
    sha256: 'bf30582eb1b012250a35b1a4a80e7dfbcf8485e7bb9de0d95efbbeef0e4ad86d',
  })
  assert.ok(!fp32.files.some((file) => file.path === 'model.int8.onnx'))
  // 固定全部已审计文件的身份清单，不依赖研究目录或在线下载。
  assert.equal(
    sha256(
      JSON.stringify(
        catalog.models.map((model) => ({
          id: model.id,
          revision: model.revision,
          files: model.files.map(({ path, bytes, sha256 }) => ({ path, bytes, sha256 })),
        })),
      ),
    ),
    'ff3a497b4ca5d884f395340b6b81d7cfc326ec80db71093d13ff20e60ff31ba4',
  )
  for (const model of catalog.models) {
    unique(model.files.map((file) => file.path))
    for (const file of model.files) {
      artifact(file)
      for (const url of file.urls ?? []) publicUrl(url)
    }
    publicUrl(model.license.url)
  }
  for (const file of fp32.files)
    assert.equal(file.urls, undefined, 'Melo installs only from the audited archive')
  const license = notices.licenses.find((entry) => entry.id === 'mit-melo')
  sameArtifact(catalogFile(fp32.id, 'LICENSE'), {
    bytes: Buffer.byteLength(license.text),
    sha256: sha256(license.text),
  })
  assert.equal(
    fp32.license.url,
    'https://huggingface.co/myshell-ai/MeloTTS-Chinese/blob/af5d207a364ea4208c6f589c89f57f88414bdd16/README.md',
  )
  assert.deepEqual(fp32.archive, {
    format: 'tar.bz2',
    bytes: 167006755,
    sha256: 'e58351ed7149f290a54534538badd4077cdbe6fddc964b24d0bee870415d1514',
    urls: [
      'https://ghfast.top/https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/vits-melo-tts-zh_en.tar.bz2',
      'https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/vits-melo-tts-zh_en.tar.bz2',
    ],
    stripPrefix: 'vits-melo-tts-zh_en/',
  })
})

test('Melo notices bind unchanged model, dictionaries and normalization resources to audited content', () => {
  const id = 'vits-melo-tts-zh_en'
  assert.equal(models.get(id).groups.length, 8)
  for (const [group, path] of [
    ['melo-weights', 'model.onnx'],
    ['melo-tokens', 'tokens.txt'],
    ['melo-user-dictionary', 'dict/user.dict.utf8'],
  ]) {
    const file = catalogFile(id, path)
    assert.ok(groups.get(group).paths.includes(file.path))
    assert.equal(models.get(id).archive.sha256, catalogModels.get(id).archive.sha256)
  }
  assert.ok(
    groups
      .get('melo-user-dictionary')
      .evidence.some((entry) => entry.sha256 === catalogFile(id, 'dict/user.dict.utf8').sha256),
  )
  assert.ok(
    groups
      .get('melo-weights')
      .evidence.some(
        (entry) =>
          entry.sha256 === '40b5b860bacbb3947cbef838719e660d2fe4961eba5b484290b4bedc0253e208',
      ),
  )
  assert.match(
    groups.get('melo-english-lexicon').summary,
    /Copyright 1998 Carnegie Mellon University/,
  )
  assert.match(
    groups.get('melo-english-lexicon').summary,
    /NOT a substitution of a modern CMUdict BSD license/,
  )
  assert.match(groups.get('melo-user-dictionary').summary, /49-byte user dictionary/)
  assert.match(groups.get('melo-cppjieba-dictionary').summary, /Nine ordinary dictionary/)
  assert.match(groups.get('melo-package-notices').summary, /not a blanket grant/)
})

test('Melo FST source matches stay pinned independently of the neural weight MIT declaration', () => {
  const fst = groups.get('melo-normalization-fsts')
  for (const name of ['date', 'number', 'phone']) {
    const source = fst.evidence.find(
      (entry) =>
        entry.url ===
        `https://hf-mirror.com/csukuangfj/icefall-tts-aishell3-vits-low-2024-04-06/resolve/57345c004e13ed640e408c4c29ab56187b18f065/data/${name}.fst`,
    )
    assert.ok(source)
    assert.equal(source.sha256, catalogFile('vits-melo-tts-zh_en', `${name}.fst`).sha256)
  }
  assert.equal(fst.status, 'pending')
  assert.match(
    fst.summary,
    /Neither the OpenFst software license nor the Melo neural-weight MIT declaration/,
  )
})
