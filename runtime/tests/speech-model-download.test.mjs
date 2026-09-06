import assert from 'node:assert/strict'
import { createHash } from 'node:crypto'
import {
  link,
  lstat,
  mkdir,
  mkdtemp,
  open,
  readFile,
  readdir,
  realpath,
  rename,
  rm,
  symlink,
  utimes,
  writeFile,
} from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { setTimeout as delay } from 'node:timers/promises'
import { SpeechModelDownloadService } from '../services/speech-model-download-service.mjs'

const digest = (value) => createHash('sha256').update(value).digest('hex')
const payload = Buffer.from('model-weights-for-tests')

// 固定 fixture 由 Python 3.14 tarfile（USTAR/PAX/GNU）+ bz2.compress 生成；测试不调用外部工具。
// bundle/ 内包含 weights/model.onnx（payload）、README.md 与 test_wavs/one.wav；坏例仅改变标注的条目。
const archiveFixtures = {
  valid:
    'QlpoOTFBWSZTWS5hkeEAAOR/gcuACABAA/+AJsLQIPfnn8ABAAiIMADZtg0ijTIwIwABP1TQNMT00DSKempgRpgBGTJgACYIpSmZNGnqepPUaYjQDQGamR+qU58rvSLzQ1wkGG+oBstRU6r9XX0YJ2JMS+uMeupJJlRSASBlb990TxQQQ62wVsSBB+TUfqUkAT3fyYHMHvSECE0vY/tMJCAgs2EBRggcJUTFd3NoGgWI0CCEySCgD122ClhZDPgkyHjoPgcLFY/SYY0wiV9EmS0GKz+0pXkkYRFxmSOC5SdVCiUS4JXUMluntUD/F3JFOFCQLmGR4Q==',
  hash: 'QlpoOTFBWSZTWUbirnAAAHH/gMqICABAAf2AAIAAQHbnnsAICCAAdRCnqGjTTQaNPUAD1PUElIAaAAAAH3rRxCCJ5CET80xFbY4UCGAy1m3WlUAn2CDIQKMHPNhgRPFSQ0SC9p7EJela1fEsZsqq6rAREPxdyRThQkEbirnA',
  missing:
    'QlpoOTFBWSZTWWGBk7AAAHF/gMmAAARAAPWAJgIYAHYHHgAICCAAVDSjT1GgPUZPUA9R6gkkQZAAAAfXRHIQbSIQirqyBbTGdAhgYhW8T2EZuBnRAaXWoZpLVVWwJ7s3LNww+N6Xd8iIgPxdyRThQkGGBk7A',
  duplicate:
    'QlpoOTFBWSZTWV2xmxQAAI7dgcqAQAP9gACAd+eewAEACAggAIkFUVGjQ0eoAAGjT1AqpT1D0gyAeoaDRtTalcGhilN8gmyIhCi9sZjcuZ1qEJEw1LHsivbkiEmYgmigi5uDauTSTIomLflQo4sjAsXqGJMaVWCq1aqfX5gyKItXPLWlUqqsUUc0QfxdyRThQkF2xmxQ',
  bomb: 'QlpoOTFBWSZTWTVCwXsEBDFbgMiAQAD3AAQAdgUeQAAIIABwUAA0DJkBUpNQ9EyZGmTanrwuICAKpEBAGfGls6ZY2kQEAUKb51StEbYNaSuOOEy20taafteed+euLYdXvfrWEBAH8XckU4UJA1QsF7A=',
  totalBomb:
    'QlpoOTFBWSZTWRbpkOUERKR7gNqAIABAAO+AAEB2BR4AAAggAJSCqKn6oepoaA/UhoBUpQyaMEGJk9Kvn291QQQQUoSnZJEgggy6/GxkyS5MMEIgQIEomUx6dFEUkhbgpCndJbnuUtVsNqS/qdNNWrhVwoq/qsAkEEHIu5IpwoSAt0yHKA==',
  absolute:
    'QlpoOTFBWSZTWUOOpe8AAF1bgMqIQALXgACAZ+bfgAgIIAB1EUeU00DIPUNBmpmoJKQMmgAANAfavIkIISIQiuFji6idqBDAYOa+FVbRNYIDGoHFm8yfaZCCJTsoQSUaEKtpOubrAVssnCqsZskg/F3JFOFCQQ46l7w=',
  traversal:
    'QlpoOTFBWSZTWbKA8KcAAHFbgMqIQAP9gBAAd+efgAgIIAB1ERpGgNGmTQAPUeUElIaNTaDUMAAh6kdXkiEHUyEIs68uPb/FCBDAYOophDNkHCcwFBIVBFmIwY+TYLApEWB8tjsXmRsGhEQ/nuCenpy0jFrcEkH8XckU4UJCygPCnA==',
  backslash:
    'QlpoOTFBWSZTWRnYa8cAAHFbgMqIQALXgAEEd+eegAgIIAB1EUyPU0GQaeo0Gh6nlBJSBkBoAAAfXTECEHUSEIf09xZPK1AhgMHN3zfIKRggm0A3F8AIYE6mTNTlsZxSbuj9kAqqpHC8Bs7exUWrFJB+LuSKcKEgM7DXjg==',
  windows:
    'QlpoOTFBWSZTWTChz2gAAHHfgMqIQAP/gAgBkAB3557ACAggAHURTQyAeoA2po0NDTEElImZJ6mEyMhiNNNpHqRg8kQghwQhHOFrjo+hqBDAYObrh5saJrBAqOIM5LzCQoSRKUcEk1uTmIW30gPB73fDUB54MsNB0YzUXpIP4u5IpwoSBhQ57QA=',
  prefix:
    'QlpoOTFBWSZTWWcov/4AAH3dgMqIQAP/gABAZ+eewAgIIAB0Gk1NABoDQAxPSCUiNGjQAAA0D71wtBAfCgghS+tpc6SNAQUChsfAtdDBGKCDTIHkywaNbOpQhAIOg5e7wbw4CPtSIHpBJqayifbJTvEV3XZkRAfi7kinChIM5Rf/wA==',
  symlink:
    'QlpoOTFBWSZTWXRAQDMAAHF/gMiAABBAAfWAAIAIADbnnsAACCAAdREnqbUGjQ09IDR6n6oJJQGgZAABpGOcgkFcyEIh3BxUUUIEMDEHvOG8a6kVLE4lNxRrlFVsR5k06M1JA09XUnzEIDdGzK2AMDB1mSSD8XckU4UJB0QEAzA=',
  hardlink:
    'QlpoOTFBWSZTWXc3BIYAAHF/gMiAABBAAeWAAIAIADbnnsAACCAAdQ1J6npB6g0ekBo9T1BJKA0G1AAAffZSCQVzIQiPTwKiihAhgYd3N27aNIqWEApQCqeUsa7DqDTozU1co+zUHs6I/EWkLUcnfCFmSSD8XckU4UJB3NwSGA=',
  fifo: 'QlpoOTFBWSZTWS/Ms4cAAG17gMiAABBAAOUAAgB3JZ4AAAggAFRCCYExMD1NQSJTQ0BoAaV7ygSDCSEIfmd5MlagQwZxE2ixiNYCqphQ8oxp9qBcAIsLREoZmRfi7kinChIF+ZZw4A==',
  marker:
    'QlpoOTFBWSZTWbP+u3YAAHHdgMqIQAP1gACAd/eegAgIIAB0Gk1GmIDRpoGgD1BKSBtRoAAA0D7hxAggPiQQQsfYpDMsiAgwGDW81u0qURWBCJkDg+6K4mFGPBZj5bN9vRp31CS1G1nCVhKUkfrKKwl7rHB7TREXkREB+LuSKcKEhZ/127A=',
  markerChild:
    'QlpoOTFBWSZTWdX1BMoAAHJdgMqIQAP9gBAAd/efgAgIIAB0GkIyA00GgGg0PUGSgbUaDQAAAPtGh4ggcwEEEU81KK2xsIBCAIFZI7jJHKCqAhikglrsHKfixKBENfFqvoLu076f9BiJZKJ+TaDYaIesXUDvnrmFsyTM1rzIiA/F3JFOFCQ1fUEygA==',
  paxValid:
    'QlpoOTFBWSZTWQlTyjwAAGZfgsqQQAP/gkBAUgB/597AAAgICCAAlAlRDU9NT1NNqepk0HqAYg9TyglFBMGkPSYBA0yGCab/TZjqiwqAGfBEQQ4kA2JCdspJCIIKgKxVp0syW+h4ru9oIYJBBCFIg2GCVDk2pvnTvp4LZXk++CgufEj26+IiVxT5zkewbnEo1xEhbYvldKq2RhjLGdhuGOQ0qzNuihpOYp2p1cBCA/i7kinChIBKnlHg',
  paxEscape:
    'QlpoOTFBWSZTWUwCdZkAAGbfgcqQQAP/gkBAUgB/59/AAQAICCAAkg1EGhoT1PUaaYaQYnqZP1R+lBJKnomxTQPU0aDIBoAaX9z7S5hnsAHnCkhGTO8aCKikjQIQyBrm0NGE0qGlyZSiCgMCLLHCMxXOVsX1jp6tmB8xcy90oHPkQLx3zJMxJ0eeyKeLyjYSTkiayjEzuPSCxaFsFAk2DVRLhAKVTM4IQB/F3JFOFCQTAJ1mQA==',
  gnuEscape:
    'QlpoOTFBWSZTWXZXAtsAAGVfwM6AQAP/gEAECQB3/57AAgAECCAAlISoKep4kzUD0g9IA0D09UEkqNMmBMjEMAQ0YTfCeLi+NgwAZvIQiXrAzBNR0IXkCGQNCZjCxnmY9czqLCERSlYYaBQ22/jvzM2UowfC+bYapd+UKrPpklalBPj8XVu54yc2BahdFI5VHSHlno0rRDccmF5O4kH8XckU4UJB2VwLbA==',
  // 连续 4097 个 PAX 头、65537 字节 GNU 扩展头、非法 PAX 长度和 size 覆盖分别验证预算及解析边界。
  metadataCount:
    'QlpoOTFBWSZTWUHdBUoBGBnfgMqQQAH/gkBAUABuA55ATAhAAlwAADGExNBgjEMjCYYwmJoMEYhkYTDGExNBgjEMjCYFKpAI9QzU0DQ0NG1OcADpAAzAAxgAYbgAMDDWiAJ4wANkADhAA3IAGUADGAB2QANsQBMCAB2wAMoAGmABn1AAygAZIAG1QAWuABgkAJygAaJACcYAGmABnAA4wANEADzgAaoAHCAB6QANEADbAA1QAOsADGABvgAcoAGtAA/IAGuAB8wAPeAB9QAOcADpAA6QAMIAH9AA3QAP2ABhAA2QAM4AHWABnAAzgAYwAM4AGyABvqAF/i7kinChIIO6CpQ=',
  metadataOversize:
    'QlpoOTFBWSZTWcjMAR8AABvfgOiAQAH3AEBEUABmAB5ACAggAGBJKn6aoP1GieoMGgoABoAAOdbS6LwhDx4MqzB3EMDTWpTNhQwLP86PTUPM2iJyt8obcaba7w973xpwgADl0gAFikyAAOhdyRThQkMjMAR8',
  paxNegative:
    'QlpoOTFBWSZTWaXV6ywAABpfgMqQQAP1AkBAUABmQF5ACIggAFRSbTU0aZqaAMnonoJRQwg000AAaceWzQewQItVxGx7UBQrjT9OUhwjFENqEHY1bAyCi4cehdtUf9vqnYTcKIxBQILXcXckU4UJCl1essA=',
  paxSize:
    'QlpoOTFBWSZTWW6gniUAABnfgMqQQAH1AkBAUABmIB5QCIggAFRSNoTJphMIepjQZKZNNNG1GgAHqb9yYiNYICwSwLKMCAgKMqh4NMYOBGtRDmfRG/JuprF06gYMeiQ3ytNyDKAJxz6F3JFOFCQbqCeJQA==',
  gnuLongValid:
    'QlpoOTFBWSZTWdcTZtQAAGRfgM6AQAP/gEAECCB3/57CBAggAJSGhUfqaNT01A9Rmo9JoMg9NQSiiYJ6CYIMAJiaPUblVG7EL9UAOIEEEKKurihPXMMgQEFQFYcCBqZVQSAxGVjQRFKTiAifRRfFOvLnA0rWxU32+cGYmDJQZfRpSYmIinwMzWb5Z2rqFYc+ge02UDe+jhVMREns7K1o06S+BED+LuSKcKEhribNqA==',
  unknownType:
    'QlpoOTFBWSZTWbf3t+oAAG/bgMqIQAL1gCIAd+efgAgIIAB1EUek0AyGnqDRoeo8oJJU9PRRkxqDBMTRppx6nVBCDZ0IRDafM71PFAhgYlKnF+jIiwgIZggwHtYTSgIHmZvSVMYn4SMGpVVPsXA2uTp1VXe9JB/F3JFOFCQt/e36gA==',
  tarTruncated:
    'QlpoOTFBWSZTWUQEi80AACDdgMqAQAH9gACAdueewAgAIABURQMh6BAaMjam9UGjVGIGmh6gAB7QCEr5i8SphBFAXbrSLgjlCBigN8qrGewJHeqEmlcEpsV48BmCp89cgAw9/xdyRThQkEQEi80=',
}

function archiveModel(data = Buffer.from(archiveFixtures.valid, 'base64')) {
  const entry = model()
  delete entry.files[0].urls
  entry.archive = {
    format: 'tar.bz2',
    bytes: data.length,
    sha256: digest(data),
    urls: ['https://cdn.example/bundle.tar.bz2'],
    stripPrefix: 'bundle/',
  }
  return entry
}

function model(id = 'sample', data = payload) {
  return {
    id,
    name: 'Test model',
    kind: 'asr',
    files: [
      {
        path: 'weights/model.onnx',
        bytes: data.length,
        sha256: digest(data),
        urls: [`https://cdn.example/${id}/weights?token=private`],
      },
    ],
  }
}

async function fixture(t, options = {}) {
  const dataDir = await mkdtemp(join(await realpath(tmpdir()), 'pisper-model-download-'))
  const services = []
  const create = (overrides = {}) => {
    const service = new SpeechModelDownloadService({
      dataDir,
      catalog: [model()],
      fetchImpl: async () => new Response(payload),
      ...options,
      ...overrides,
    })
    services.push(service)
    return service
  }
  t.after(async () => {
    await Promise.all(services.map((service) => service.dispose()))
    await rm(dataDir, { recursive: true, force: true })
  })
  return { dataDir, service: create(), create }
}

async function until(predicate) {
  const end = Date.now() + 4000
  while (!(await predicate())) {
    if (Date.now() > end) assert.fail('Timed out waiting for test state')
    await delay(2)
  }
}

function controlledBody() {
  let controller
  let cancelled = false
  const stream = new ReadableStream({
    start(value) {
      controller = value
    },
    cancel() {
      cancelled = true
    },
  })
  return {
    stream,
    push: (value) => controller.enqueue(value),
    close: () => controller.close(),
    get cancelled() {
      return cancelled
    },
  }
}

async function partialDirectory(dataDir) {
  const root = join(dataDir, 'speech-models')
  const name = (await readdir(root)).find((entry) => entry.endsWith('.partial'))
  assert.ok(name)
  return join(root, name)
}

test('no automatic download; streams exact multi-file progress and persists verified installation', async (t) => {
  const body = controlledBody()
  const entry = model()
  const tokens = Buffer.from('a\nb\n')
  entry.files.push({
    path: 'tokens.txt',
    bytes: tokens.length,
    sha256: digest(tokens),
    urls: ['https://cdn.example/tokens'],
  })
  let calls = 0
  const { service, create, dataDir } = await fixture(t, {
    catalog: [entry],
    fetchImpl: async (_url, options) => {
      calls += 1
      assert.equal(options.redirect, 'manual')
      assert.equal(options.headers['Accept-Encoding'], 'identity')
      return calls === 1 ? new Response(body.stream) : new Response(tokens)
    },
  })
  assert.equal((await service.status('sample')).status, 'not-installed')
  assert.equal((await service.list())[0].name, 'Test model')
  assert.equal(calls, 0)
  const started = await service.startDownload('sample')
  assert.equal(started.status, 'downloading')
  assert.equal(started.totalBytes, payload.length + tokens.length)
  await until(() => calls === 1)
  body.push(payload.subarray(0, 7))
  await until(async () => (await service.status('sample')).downloadedBytes === 7)
  assert.equal(service.inflight.size, 1)
  body.push(payload.subarray(7))
  body.close()
  const done = await service.download('sample')
  assert.equal(done.status, 'installed')
  assert.equal(done.downloadedBytes, done.totalBytes)
  assert.equal(service.inflight.size, 0)
  const directory = await service.modelDirectory('sample')
  assert.equal(directory, join(dataDir, 'speech-models', 'sample'))
  assert.deepEqual(await readFile(join(directory, 'weights/model.onnx')), payload)
  const marker = JSON.parse(await readFile(join(directory, '.installation.json'), 'utf8'))
  assert.equal(marker.id, 'sample')
  assert.match(marker.fingerprint, /^[a-f0-9]{64}$/)
  assert.equal(marker.files.length, 2)
  assert.deepEqual(await readdir(join(dataDir, 'speech-models')), ['sample'])
  const fresh = create({ fetchImpl: () => assert.fail('persistent installation must not fetch') })
  assert.equal((await fresh.status('sample')).status, 'installed')
  assert.equal(await fresh.modelDirectory('sample'), directory)
  assert.equal((await fresh.download('sample')).status, 'installed')
  assert.equal(JSON.stringify(await fresh.list()).includes('private'), false)
})

test('does not trust marker alone: identity, manifest and every file are rechecked', async (t) => {
  const { service } = await fixture(t)
  await service.download('sample')
  const directory = await service.modelDirectory('sample')
  const markerPath = join(directory, '.installation.json')
  const original = await readFile(markerPath, 'utf8')
  for (const change of [{ id: 'different' }, { fingerprint: '0'.repeat(64) }, { files: [] }]) {
    await writeFile(markerPath, JSON.stringify({ ...JSON.parse(original), ...change }))
    assert.equal((await service.status('sample')).status, 'not-installed')
    await assert.rejects(service.modelDirectory('sample'), /not installed/)
  }
  await writeFile(markerPath, original)
  await writeFile(join(directory, 'weights/model.onnx'), Buffer.alloc(payload.length))
  assert.equal((await service.status('sample')).status, 'not-installed')
  await assert.rejects(service.modelDirectory('sample'), /not installed/)
  await rm(join(directory, 'weights/model.onnx'))
  await assert.rejects(service.modelDirectory('sample'), /not installed/)
})

async function trackContentReads(t, path) {
  const handle = await open(path)
  const prototype = Object.getPrototypeOf(handle)
  await handle.close()
  const read = prototype.read
  const measured = { calls: 0, bytes: 0 }
  t.mock.method(prototype, 'read', async function (...args) {
    const result = await read.apply(this, args)
    measured.calls += 1
    measured.bytes += result.bytesRead
    return result
  })
  return measured
}

test('verified installation cache skips all content reads for status, list and modelDirectory', async (t) => {
  const data = Buffer.alloc(2 * 1024 * 1024, 17)
  const { service, create, dataDir } = await fixture(t, {
    catalog: [model('sample', data)],
    fetchImpl: async () => new Response(data),
  })
  await service.download('sample')
  const directory = join(dataDir, 'speech-models', 'sample')
  const reads = await trackContentReads(t, join(directory, 'weights/model.onnx'))
  assert.equal((await service.status('sample')).status, 'installed')
  assert.ok(reads.bytes >= data.length)
  const verifiedBytes = reads.bytes
  const verifiedCalls = reads.calls
  for (let index = 0; index < 3; index += 1) {
    assert.equal((await service.status('sample')).status, 'installed')
    assert.equal(await service.modelDirectory('sample'), directory)
    assert.equal((await service.list())[0].status, 'installed')
  }
  assert.equal(reads.bytes, verifiedBytes)
  assert.equal(reads.calls, verifiedCalls)
  const fresh = create()
  assert.equal((await fresh.status('sample')).status, 'installed')
  assert.ok(reads.bytes >= verifiedBytes + data.length)
})

test('unrelated files in shared ancestors do not invalidate a verified installation', async (t) => {
  const { service, dataDir } = await fixture(t)
  await service.download('sample')
  const original = service.verifyInstallationContents.bind(service)
  let count = 0
  service.verifyInstallationContents = async (...args) => {
    const result = await original(...args)
    await mkdir(join(dataDir, `other-session-${++count}`))
    return result
  }
  assert.equal((await service.status('sample')).status, 'installed')
  const verified = count
  await writeFile(join(dataDir, 'unrelated-settings.json'), '{}')
  assert.equal((await service.status('sample')).status, 'installed')
  assert.equal(count, verified)
  assert.equal(await service.modelDirectory('sample'), join(dataDir, 'speech-models', 'sample'))
})

test('list caches installed models even while other catalog models are not installed', async (t) => {
  const { service, dataDir } = await fixture(t, {
    catalog: [model(), model('missing')],
  })
  await service.download('sample')
  const reads = await trackContentReads(
    t,
    join(dataDir, 'speech-models', 'sample', 'weights/model.onnx'),
  )
  assert.deepEqual(
    (await service.list()).map((entry) => entry.status),
    ['installed', 'not-installed'],
  )
  assert.ok(reads.bytes >= payload.length)
  const verifiedBytes = reads.bytes
  for (let index = 0; index < 3; index += 1) {
    assert.deepEqual(
      (await service.list()).map((entry) => entry.status),
      ['installed', 'not-installed'],
    )
  }
  assert.equal(reads.bytes, verifiedBytes)
})

for (const restoreMtime of [false, true]) {
  test(`cached proof rejects same-size corruption with restored mtime=${restoreMtime}`, async (t) => {
    const { service } = await fixture(t)
    await service.download('sample')
    const directory = await service.modelDirectory('sample')
    const path = join(directory, 'weights/model.onnx')
    const before = await lstat(path, { bigint: true })
    await delay(20)
    await writeFile(path, Buffer.alloc(payload.length))
    if (restoreMtime) {
      await utimes(path, Number(before.atimeNs) / 1e9, Number(before.mtimeNs) / 1e9)
    }
    const after = await lstat(path, { bigint: true })
    assert.equal(after.size, before.size)
    assert.notEqual(after.ctimeNs, before.ctimeNs)
    const reads = await trackContentReads(t, path)
    assert.equal((await service.status('sample')).status, 'not-installed')
    assert.ok(reads.bytes >= payload.length)
    await assert.rejects(service.modelDirectory('sample'), /not installed/)
    await writeFile(path, payload)
    assert.equal(await service.modelDirectory('sample'), directory)
    const verifiedBytes = reads.bytes
    assert.equal((await service.status('sample')).status, 'installed')
    assert.equal(reads.bytes, verifiedBytes)
  })
}

for (const target of ['weights/model.onnx', '.installation.json', 'weights', 'model', 'root']) {
  test(`cached proof rehashes after safe replacement of ${target}`, async (t) => {
    const { service, dataDir } = await fixture(t)
    await service.download('sample')
    const directory = await service.modelDirectory('sample')
    const path =
      target === 'root'
        ? join(dataDir, 'speech-models')
        : target === 'model'
          ? directory
          : join(directory, target)
    const old = `${path}.old`
    const reads = await trackContentReads(t, join(directory, 'weights/model.onnx'))
    await rename(path, old)
    if (target === 'root' || target === 'model' || target === 'weights') {
      await mkdir(path)
      for (const entry of await readdir(old)) await rename(join(old, entry), join(path, entry))
      await rm(old, { recursive: true })
    } else {
      await writeFile(path, await readFile(old))
      await rm(old)
    }
    assert.equal(await service.modelDirectory('sample'), directory)
    assert.ok(reads.bytes >= payload.length)
    const verifiedBytes = reads.bytes
    assert.equal((await service.status('sample')).status, 'installed')
    assert.equal(reads.bytes, verifiedBytes)
  })
}

test('marker-only attribute changes force full hashing and invalid marker content clears the proof', async (t) => {
  const { service } = await fixture(t)
  await service.download('sample')
  const directory = await service.modelDirectory('sample')
  const path = join(directory, '.installation.json')
  const marker = await readFile(path)
  const reads = await trackContentReads(t, path)
  const changedTime = new Date(Date.now() + 10_000)
  await utimes(path, changedTime, changedTime)
  assert.equal((await service.status('sample')).status, 'installed')
  assert.ok(reads.bytes >= marker.length + payload.length)
  await writeFile(path, Buffer.alloc(marker.length, 32))
  await assert.rejects(service.modelDirectory('sample'), /not installed/)
  await writeFile(path, marker)
  const before = reads.bytes
  assert.equal((await service.status('sample')).status, 'installed')
  assert.ok(reads.bytes >= before + marker.length + payload.length)
})

for (const target of ['weights/model.onnx', '.installation.json']) {
  test(`cached proof rejects newly hard-linked ${target}`, async (t) => {
    const { service, dataDir } = await fixture(t)
    await service.download('sample')
    const directory = await service.modelDirectory('sample')
    const outside = join(dataDir, 'external-link')
    await link(join(directory, target), outside)
    assert.equal((await service.status('sample')).status, 'not-installed')
    await assert.rejects(service.modelDirectory('sample'), /not installed/)
    await assert.rejects(service.download('sample'), { code: 'path' })
    await rm(outside)
    assert.equal(await service.modelDirectory('sample'), directory)
  })
}

for (const target of ['root', 'model', 'nested']) {
  test(`cached proof rejects ${target} directory replacement with a symlink/junction`, async (t) => {
    const { service, dataDir } = await fixture(t)
    await service.download('sample')
    const directory = await service.modelDirectory('sample')
    const path =
      target === 'root'
        ? join(dataDir, 'speech-models')
        : target === 'model'
          ? directory
          : join(directory, 'weights')
    const outside = join(dataDir, 'detached')
    await rename(path, outside)
    await symlink(outside, path, process.platform === 'win32' ? 'junction' : 'dir')
    assert.equal((await service.status('sample')).status, 'not-installed')
    await assert.rejects(service.modelDirectory('sample'), /not installed/)
    await assert.rejects(service.download('sample'), { code: 'path' })
    const originalFile =
      target === 'root'
        ? join(outside, 'sample/weights/model.onnx')
        : target === 'model'
          ? join(outside, 'weights/model.onnx')
          : join(outside, 'model.onnx')
    assert.deepEqual(await readFile(originalFile), payload)
  })
}

test('cache invalidates on catalog fingerprint and manifest changes', async (t) => {
  const { service } = await fixture(t)
  await service.download('sample')
  const directory = await service.modelDirectory('sample')
  const entry = service.getModel('sample')
  const originalFingerprint = entry.fingerprint
  entry.fingerprint = '0'.repeat(64)
  await assert.rejects(service.modelDirectory('sample'), /not installed/)
  entry.fingerprint = originalFingerprint
  assert.equal(await service.modelDirectory('sample'), directory)
  const sha256 = entry.files[0].sha256
  entry.files[0].sha256 = '0'.repeat(64)
  await assert.rejects(service.modelDirectory('sample'), /not installed/)
  entry.files[0].sha256 = sha256
  const reads = await trackContentReads(t, join(directory, 'weights/model.onnx'))
  assert.equal(await service.modelDirectory('sample'), directory)
  assert.ok(reads.bytes >= payload.length)
})

test('download attempts, failure, reinstall and dispose invalidate cached proofs', async (t) => {
  const { service } = await fixture(t)
  await service.download('sample')
  const directory = await service.modelDirectory('sample')
  const path = join(directory, 'weights/model.onnx')
  const reads = await trackContentReads(t, path)
  assert.equal((await service.download('sample')).status, 'installed')
  assert.ok(reads.bytes >= payload.length)
  await writeFile(path, Buffer.alloc(payload.length))
  service.fetchImpl = async () => new Response('failed', { status: 503 })
  await assert.rejects(service.download('sample'), { code: 'http' })
  await assert.rejects(service.modelDirectory('sample'), /not installed/)
  service.fetchImpl = async () => new Response(payload)
  assert.equal((await service.download('sample')).status, 'installed')
  let before = reads.bytes
  assert.equal(await service.modelDirectory('sample'), directory)
  assert.ok(reads.bytes >= before + payload.length)
  before = reads.bytes
  assert.equal((await service.status('sample')).status, 'installed')
  assert.equal(reads.bytes, before)
  await service.dispose()
  assert.equal(await service.modelDirectory('sample'), directory)
  assert.ok(reads.bytes >= before + payload.length)
  before = reads.bytes
  assert.equal((await service.status('sample')).status, 'installed')
  assert.ok(reads.bytes >= before + payload.length)
})

test('dispose during full verification prevents late cache population', async (t) => {
  const { service, dataDir } = await fixture(t)
  await service.download('sample')
  const path = join(dataDir, 'speech-models', 'sample', 'weights/model.onnx')
  let release
  let entered = false
  const gate = new Promise((resolve) => {
    release = resolve
  })
  const verify = service.verifyInstallationContents.bind(service)
  t.mock.method(service, 'verifyInstallationContents', async (...args) => {
    const result = await verify(...args)
    entered = true
    await gate
    return result
  })
  const pending = service.status('sample')
  await until(() => entered)
  await service.dispose()
  release()
  assert.equal((await pending).status, 'installed')
  const reads = await trackContentReads(t, path)
  assert.equal((await service.status('sample')).status, 'installed')
  assert.ok(reads.bytes >= payload.length)
})

test('files changed during full verification never produce a cached proof', async (t) => {
  const { service } = await fixture(t)
  await service.download('sample')
  const directory = await service.modelDirectory('sample')
  const path = join(directory, 'weights/model.onnx')
  const verify = service.verifyInstallationContents.bind(service)
  t.mock.method(service, 'verifyInstallationContents', async (...args) => {
    const result = await verify(...args)
    await writeFile(path, Buffer.alloc(payload.length))
    return result
  })
  const future = new Date(Date.now() + 10_000)
  await utimes(path, future, future)
  await assert.rejects(service.modelDirectory('sample'), /not installed/)
  await assert.rejects(service.modelDirectory('sample'), /not installed/)
})

for (const [name, response, code] of [
  ['bad hash', () => new Response(Buffer.alloc(payload.length)), 'integrity'],
  ['truncated stream', () => new Response(payload.subarray(0, 4)), 'size'],
  ['oversized stream', () => new Response(Buffer.concat([payload, Buffer.from('!')])), 'size'],
  [
    'oversized declared length',
    () => new Response(payload, { headers: { 'content-length': String(payload.length + 1) } }),
    'size',
  ],
  [
    'truncated declared length',
    () => new Response(payload, { headers: { 'content-length': '4' } }),
    'size',
  ],
  [
    'noncanonical declared length',
    () => new Response(payload, { headers: { 'content-length': `+${payload.length}` } }),
    'size',
  ],
  [
    'compressed response',
    () => new Response(payload, { headers: { 'content-encoding': 'gzip' } }),
    'size',
  ],
  [
    'unsolicited partial response',
    () =>
      new Response(payload, {
        status: 206,
        headers: { 'content-range': `bytes 0-${payload.length - 1}/${payload.length}` },
      }),
    'range',
  ],
  ['HTTP failure', () => new Response('private credentials', { status: 403 }), 'http'],
]) {
  test(`rejects ${name} without installed marker or inflight task`, async (t) => {
    const { service, dataDir } = await fixture(t, { fetchImpl: async () => response() })
    await assert.rejects(service.download('sample'), (error) => error.code === code)
    const state = await service.status('sample')
    assert.equal(state.status, 'error')
    assert.ok(state.downloadedBytes <= state.totalBytes)
    assert.equal(state.error.includes('private'), false)
    assert.equal(service.inflight.size, 0)
    await assert.rejects(service.modelDirectory('sample'), /not installed/)
    const staging = await partialDirectory(dataDir)
    await assert.rejects(readFile(join(staging, '.installation.json')), { code: 'ENOENT' })
  })
}

test('cancel waits for file closure, retry resumes strict Range and updates exact progress', async (t) => {
  const body = controlledBody()
  let calls = 0
  let signal
  const { service, dataDir } = await fixture(t, {
    fetchImpl: async (_url, options) => {
      calls += 1
      signal = options.signal
      if (calls === 1) return new Response(body.stream)
      assert.equal(options.headers.Range, 'bytes=6-')
      return new Response(payload.subarray(6), {
        status: 206,
        headers: {
          'content-range': `bytes 6-${payload.length - 1}/${payload.length}`,
          'content-length': String(payload.length - 6),
        },
      })
    },
  })
  await service.startDownload('sample')
  await until(() => calls === 1)
  body.push(payload.subarray(0, 6))
  await until(async () => (await service.status('sample')).downloadedBytes === 6)
  assert.equal((await service.cancelDownload('sample')).status, 'cancelled')
  assert.equal(signal.aborted, true)
  assert.equal(body.cancelled, true)
  assert.equal(service.inflight.size, 0)
  const staging = await partialDirectory(dataDir)
  assert.deepEqual(await readFile(join(staging, 'weights/model.onnx')), payload.subarray(0, 6))
  assert.equal((await service.download('sample')).status, 'installed')
  assert.equal(calls, 2)
  assert.equal(service.inflight.size, 0)
})

test('retry after process recreation resumes an interrupted prefix; ignored Range restarts from zero', async (t) => {
  const { service, create } = await fixture(t, {
    fetchImpl: async () => new Response(payload.subarray(0, 5)),
  })
  await assert.rejects(service.download('sample'), { code: 'size' })
  await service.dispose()
  let calls = 0
  const fresh = create({
    fetchImpl: async (_url, options) => {
      calls += 1
      assert.equal(options.headers.Range, 'bytes=5-')
      return new Response(payload, { headers: { 'content-length': String(payload.length) } })
    },
  })
  assert.equal((await fresh.download('sample')).status, 'installed')
  assert.equal(calls, 1)
  assert.deepEqual(
    await readFile(join(await fresh.modelDirectory('sample'), 'weights/model.onnx')),
    payload,
  )
})

for (const range of ['bytes 0-21/22', 'bytes 5-20/22', 'bytes 5-21/*', 'bytes 5-21/23', null]) {
  test(`rejects mismatched resumed Content-Range ${range}`, async (t) => {
    let calls = 0
    const { service, dataDir } = await fixture(t, {
      fetchImpl: async () => {
        calls += 1
        return calls === 1
          ? new Response(payload.subarray(0, 5))
          : new Response(payload.subarray(5), {
              status: 206,
              headers: range ? { 'content-range': range } : {},
            })
      },
    })
    await assert.rejects(service.download('sample'), { code: 'size' })
    await assert.rejects(service.download('sample'), { code: 'range' })
    assert.deepEqual(
      await readFile(join(await partialDirectory(dataDir), 'weights/model.onnx')),
      payload.subarray(0, 5),
    )
    assert.equal(service.inflight.size, 0)
  })
}

test('same-model requests deduplicate and different models run serially', async (t) => {
  const body = controlledBody()
  const calls = []
  const { service } = await fixture(t, {
    catalog: [model('one'), model('two')],
    fetchImpl: async (url) => {
      calls.push(url)
      return calls.length === 1 ? new Response(body.stream) : new Response(payload)
    },
  })
  const first = service.download('one')
  const duplicate = service.download('one')
  const other = service.download('two')
  await service.startDownload('one')
  await until(() => calls.length === 1)
  assert.equal(service.inflight.size, 2)
  assert.equal((await service.status('two')).downloadedBytes, 0)
  body.push(payload)
  body.close()
  const results = await Promise.all([first, duplicate, other])
  assert.ok(results.every((result) => result.status === 'installed'))
  assert.equal(calls.length, 2)
  assert.equal(service.inflight.size, 0)
})

test('queued cancellation finishes without waiting for another model and never later starts it', async (t) => {
  const body = controlledBody()
  let calls = 0
  const { service } = await fixture(t, {
    catalog: [model('one'), model('two')],
    fetchImpl: async () => {
      calls += 1
      return new Response(body.stream)
    },
  })
  await service.startDownload('one')
  await service.startDownload('two')
  await until(() => calls === 1)
  assert.equal((await service.cancelDownload('two')).status, 'cancelled')
  assert.equal(service.inflight.size, 1)
  body.push(payload)
  body.close()
  await service.download('one')
  assert.equal(calls, 1)
  assert.equal(service.inflight.size, 0)
})

test('global queue serializes multiple service instances and dispose aborts only owned work', async (t) => {
  const body = controlledBody()
  let calls = 0
  const { service, create } = await fixture(t, {
    fetchImpl: async () => {
      calls += 1
      return new Response(body.stream)
    },
  })
  const other = create({
    catalog: [model('two')],
    fetchImpl: async () => {
      calls += 1
      return new Response(payload)
    },
  })
  await service.startDownload('sample')
  await until(() => calls === 1)
  const next = other.download('two')
  await service.dispose()
  assert.equal((await service.status('sample')).status, 'cancelled')
  assert.equal((await next).status, 'installed')
  assert.equal(service.inflight.size, 0)
  assert.equal(other.inflight.size, 0)
  await assert.rejects(service.download('sample'), { code: 'disposed' })
})

test('catalog and unknown IDs reject traversal, platform aliases and arbitrary URLs', async (t) => {
  const { dataDir, service } = await fixture(t)
  for (const id of [
    '../escape',
    '/escape',
    'a\\b',
    'C:escape',
    '.',
    '..',
    'a.',
    'CON',
    'aux.txt',
    '',
    null,
  ]) {
    assert.throws(() => new SpeechModelDownloadService({ dataDir, catalog: [model(id)] }), {
      code: 'catalog',
    })
    await assert.rejects(service.download(id), { code: 'unknown' })
    await assert.rejects(service.status(id), { code: 'unknown' })
    await assert.rejects(service.modelDirectory(id), { code: 'unknown' })
    await assert.rejects(service.cancelDownload(id), { code: 'unknown' })
  }
  for (const path of [
    '../a',
    'a/../b',
    '/a',
    'C:/a',
    'a\\b',
    'a//b',
    'a/./b',
    '.installation.json',
    'a:stream',
    'a./b',
    'NUL.bin',
    'COM\u00b9.txt',
    'CONOUT$',
    '.installation.json/file',
    'a\u0085b',
    'a\u0000b',
  ]) {
    const entry = model()
    entry.files[0].path = path
    assert.throws(() => new SpeechModelDownloadService({ dataDir, catalog: [entry] }), {
      code: 'catalog',
    })
  }
  for (const url of [
    'http://cdn.example/file',
    'file:///etc/passwd',
    'https://user:secret@cdn.example/file',
    'https://cdn.example/file#fragment',
  ]) {
    const entry = model()
    entry.files[0].urls = [url]
    assert.throws(() => new SpeechModelDownloadService({ dataDir, catalog: [entry] }), {
      code: 'catalog',
    })
  }
  for (const mutate of [
    (entry) => {
      entry.files[0].bytes = Infinity
    },
    (entry) => {
      entry.files[0].bytes = -1
    },
    (entry) => {
      entry.files[0].sha256 = 'bad'
    },
    (entry) => {
      entry.files.push({ ...entry.files[0], path: 'WEIGHTS/model.onnx' })
    },
    (entry) => {
      entry.files.push({ ...entry.files[0], path: 'weights' })
    },
    (entry) => {
      entry.kind = 'script'
    },
  ]) {
    const entry = model()
    mutate(entry)
    assert.throws(() => new SpeechModelDownloadService({ dataDir, catalog: [entry] }), {
      code: 'catalog',
    })
  }
})

test('legal CDN resource paths support punctuation, internal spaces and encoded URL segments', async (t) => {
  const entry = model()
  entry.files[0].path = 'espeak-ng-data/voices/!v/Mr serious'
  entry.files[0].urls = [
    'https://hf-mirror.com/org/model/resolve/main/espeak-ng-data/voices/%21v/Mr%20serious',
  ]
  const { service } = await fixture(t, {
    catalog: [entry],
    fetchImpl: async (url) => {
      assert.equal(url, entry.files[0].urls[0])
      return new Response(payload)
    },
  })
  assert.equal((await service.download('sample')).status, 'installed')
  assert.deepEqual(
    await readFile(join(await service.modelDirectory('sample'), entry.files[0].path)),
    payload,
  )
})

test('catalog input is snapshotted and public output cannot mutate the trusted manifest', async (t) => {
  const entry = model()
  const { service } = await fixture(t, {
    catalog: [entry],
    fetchImpl: async (url) => {
      assert.equal(url.startsWith('https://cdn.example/'), true)
      return new Response(payload)
    },
  })
  entry.files[0].urls[0] = 'https://untrusted.example/file'
  entry.files[0].bytes = 0
  const list = await service.list()
  list[0].files[0].bytes = 999
  list[0].downloadedBytes = 999
  assert.equal((await service.download('sample')).status, 'installed')
})

for (const place of ['root', 'model', 'nested', 'marker']) {
  test(`rejects ${place} directory symlink/junction without touching external files`, async (t) => {
    const { service, dataDir } = await fixture(t)
    const outside = join(dataDir, 'outside')
    await mkdir(outside)
    await writeFile(join(outside, 'keep.txt'), 'unchanged')
    const root = join(dataDir, 'speech-models')
    let destination
    if (place === 'root') destination = root
    else {
      await mkdir(root)
      if (place === 'model') destination = join(root, 'sample')
      else {
        await service.download('sample')
        const installed = await service.modelDirectory('sample')
        destination = join(installed, place === 'marker' ? '.installation.json' : 'weights')
        await rm(destination, { recursive: true })
      }
    }
    await symlink(outside, destination, process.platform === 'win32' ? 'junction' : 'dir')
    await assert.rejects(service.modelDirectory('sample'), /not installed/)
    await assert.rejects(service.download('sample'), { code: 'path' })
    assert.equal(await readFile(join(outside, 'keep.txt'), 'utf8'), 'unchanged')
    assert.deepEqual(await readdir(outside), ['keep.txt'])
    assert.equal(service.inflight.size, 0)
  })
}

test('rejects hard-linked model files and staged prefixes', async (t) => {
  const { service, dataDir } = await fixture(t, {
    fetchImpl: async () => new Response(payload.subarray(0, 5)),
  })
  await assert.rejects(service.download('sample'), { code: 'size' })
  const outside = join(dataDir, 'outside.bin')
  await writeFile(outside, payload)
  const path = join(await partialDirectory(dataDir), 'weights/model.onnx')
  await rm(path)
  await link(outside, path)
  await assert.rejects(service.download('sample'), { code: 'path' })
  assert.deepEqual(await readFile(outside), payload)
  assert.equal(service.inflight.size, 0)
})

test('a failed new manifest leaves old verified installation intact; successful replacement is complete', async (t) => {
  const { service, create } = await fixture(t)
  await service.download('sample')
  const directory = await service.modelDirectory('sample')
  const originalMarker = await readFile(join(directory, '.installation.json'), 'utf8')
  const newData = Buffer.from('new-version-of-model')
  let healthy = false
  const newer = create({
    catalog: [model('sample', newData)],
    fetchImpl: async () => new Response(healthy ? newData : Buffer.alloc(newData.length)),
  })
  await assert.rejects(newer.download('sample'), { code: 'integrity' })
  assert.equal(await service.modelDirectory('sample'), directory)
  assert.deepEqual(await readFile(join(directory, 'weights/model.onnx')), payload)
  assert.equal(await readFile(join(directory, '.installation.json'), 'utf8'), originalMarker)
  healthy = true
  assert.equal((await newer.download('sample')).status, 'installed')
  assert.deepEqual(
    await readFile(join(await newer.modelDirectory('sample'), 'weights/model.onnx')),
    newData,
  )
  await assert.rejects(service.modelDirectory('sample'), /not installed/)
})

test('only declared HTTPS fallback redirects are followed; bounded loops and errors release bodies', async (t) => {
  const entry = model()
  entry.files[0].urls.push('https://fallback.example/model')
  const calls = []
  const { service, create } = await fixture(t, {
    catalog: [entry],
    fetchImpl: async (url, options) => {
      calls.push(url)
      assert.equal(options.redirect, 'manual')
      return url.includes('cdn.example')
        ? new Response(null, { status: 302, headers: { location: entry.files[0].urls[1] } })
        : new Response(payload)
    },
  })
  assert.equal((await service.download('sample')).status, 'installed')
  assert.equal(calls.length, 2)
  for (const location of [
    'http://cdn.example/no',
    'https://evil.example/?secret=private',
    'file:///etc/passwd',
  ]) {
    let requested = 0
    const bad = create({
      catalog: [model('bad')],
      fetchImpl: async () => {
        requested += 1
        return new Response('secret', { status: 302, headers: { location } })
      },
    })
    await assert.rejects(bad.download('bad'), { code: 'redirect' })
    assert.equal(requested, 1)
    assert.equal((await bad.status('bad')).error.includes('private'), false)
    assert.equal(bad.inflight.size, 0)
  }
  const loopEntry = model('loop')
  let loopCalls = 0
  const loop = create({
    catalog: [loopEntry],
    fetchImpl: async () => {
      loopCalls += 1
      return new Response(null, { status: 302, headers: { location: loopEntry.files[0].urls[0] } })
    },
  })
  await assert.rejects(loop.download('loop'), { code: 'redirect' })
  assert.equal(loopCalls, 3)
})

test('explicit trusted CDN redirects preserve Range but never persist short-lived signed URLs', async (t) => {
  const entry = model()
  let attempt = 0
  const requested = []
  const { service } = await fixture(t, {
    catalog: [entry],
    trustedRedirectHosts: ['cas-bridge.xethub.hf.co'],
    fetchImpl: async (url, options) => {
      requested.push(url)
      assert.equal(options.credentials, 'omit')
      assert.equal(options.referrerPolicy, 'no-referrer')
      if (url === entry.files[0].urls[0]) {
        attempt += 1
        return new Response(null, {
          status: 302,
          headers: { location: `https://cas-bridge.xethub.hf.co/blob?Signature=secret-${attempt}` },
        })
      }
      if (attempt === 1) return new Response(payload.subarray(0, 3))
      assert.equal(options.headers.Range, 'bytes=3-')
      return new Response(payload.subarray(3), {
        status: 206,
        headers: { 'content-range': `bytes 3-${payload.length - 1}/${payload.length}` },
      })
    },
  })
  await assert.rejects(service.download('sample'), { code: 'size' })
  assert.equal((await service.download('sample')).status, 'installed')
  assert.equal(requested.length, 4)
  assert.equal(requested[2], entry.files[0].urls[0])
  assert.notEqual(requested[1], requested[3])
  const marker = await readFile(
    join(await service.modelDirectory('sample'), '.installation.json'),
    'utf8',
  )
  assert.equal(marker.includes('Signature'), false)
  assert.equal(JSON.stringify(await service.list()).includes('secret'), false)
})

test('trusted redirect hosts remain exact, public HTTPS hosts without ports or credentials', async (t) => {
  const { dataDir, create } = await fixture(t)
  for (const host of [
    'localhost',
    '127.0.0.1',
    '[::1]',
    '*.hf.co',
    'x.local',
    'x.internal',
    'cas-bridge.xethub.hf.co:8443',
    'https://cas-bridge.xethub.hf.co',
  ]) {
    assert.throws(
      () => new SpeechModelDownloadService({ dataDir, catalog: [], trustedRedirectHosts: [host] }),
      { code: 'catalog' },
    )
  }
  for (const location of [
    'https://127.0.0.1/file',
    'https://10.0.0.1/file',
    'https://169.254.169.254/file',
    'https://[::1]/file',
    'https://localhost/file',
    'https://cas-bridge.xethub.hf.co.evil.example/file',
    'http://cas-bridge.xethub.hf.co/file',
    'https://cas-bridge.xethub.hf.co:8443/file',
    'https://user:secret@cas-bridge.xethub.hf.co/file',
  ]) {
    let calls = 0
    const service = create({
      trustedRedirectHosts: ['cas-bridge.xethub.hf.co'],
      fetchImpl: async () => {
        calls += 1
        return new Response(null, { status: 302, headers: { location } })
      },
    })
    await assert.rejects(service.download('sample'), { code: 'redirect' })
    assert.equal(calls, 1)
    assert.equal(service.inflight.size, 0)
  }
})

test('HTTP failure uses only the registered fallback and cancels rejected response streams', async (t) => {
  const entry = model()
  entry.files[0].urls.push('https://fallback.example/model')
  const rejected = controlledBody()
  let calls = 0
  const { service } = await fixture(t, {
    catalog: [entry],
    fetchImpl: async () => {
      calls += 1
      return calls === 1 ? new Response(rejected.stream, { status: 503 }) : new Response(payload)
    },
  })
  assert.equal((await service.download('sample')).status, 'installed')
  assert.equal(rejected.cancelled, true)
  assert.equal(calls, 2)
})

test('background fetch rejection is handled and error details do not expose credentials', async (t) => {
  const { service } = await fixture(t, {
    fetchImpl: async () => {
      throw new Error('https://user:password@cdn.example/?token=private')
    },
  })
  const started = await service.startDownload('sample')
  assert.equal(started.status, 'downloading')
  await until(() => service.inflight.size === 0)
  const status = await service.status('sample')
  assert.equal(status.status, 'error')
  assert.equal(status.error, 'Speech model CDN request failed.')
})

test('fetch and stalled body timeouts abort resources and permit retry', async (t) => {
  let captured
  let resolveFetch
  const { service, create } = await fixture(t, {
    requestTimeoutMs: 20,
    fetchImpl: (_url, { signal }) => {
      captured = signal
      return new Promise((resolve) => {
        resolveFetch = resolve
      })
    },
  })
  await assert.rejects(service.download('sample'), { code: 'timeout' })
  assert.equal(captured.aborted, true)
  assert.equal(service.inflight.size, 0)
  const late = controlledBody()
  resolveFetch(new Response(late.stream))
  await until(() => late.cancelled)
  const body = controlledBody()
  const stalled = create({
    catalog: [model('stalled')],
    requestTimeoutMs: 20,
    fetchImpl: async () => new Response(body.stream),
  })
  await assert.rejects(stalled.download('stalled'), { code: 'timeout' })
  assert.equal(body.cancelled, true)
  assert.equal(stalled.inflight.size, 0)
})

test('dispose during pending fetch ends tasks; late rejecting fetch has no unhandled rejection', async (t) => {
  let rejectFetch
  const { service } = await fixture(t, {
    fetchImpl: () =>
      new Promise((_, reject) => {
        rejectFetch = reject
      }),
  })
  await service.startDownload('sample')
  await until(() => Boolean(rejectFetch))
  await service.dispose()
  assert.equal(service.inflight.size, 0)
  assert.equal((await service.status('sample')).status, 'cancelled')
  rejectFetch(new Error('late credentials'))
  await delay(5)
})

test('stream network failure falls back with the persisted prefix and strict Range', async (t) => {
  const entry = model()
  entry.files[0].urls.push('https://fallback.example/model')
  let calls = 0
  const body = controlledBody()
  let failStream
  const firstStream = new ReadableStream({
    start(controller) {
      controller.enqueue(payload.subarray(0, 3))
      failStream = () => controller.error(new Error('network secret'))
    },
  })
  const { service } = await fixture(t, {
    catalog: [entry],
    fetchImpl: async (_url, options) => {
      calls += 1
      if (calls === 1) return new Response(firstStream)
      assert.equal(options.headers.Range, 'bytes=3-')
      return new Response(body.stream, {
        status: 206,
        headers: { 'content-range': `bytes 3-${payload.length - 1}/${payload.length}` },
      })
    },
  })
  const finished = service.download('sample')
  await until(async () => (await service.status('sample')).downloadedBytes === 3)
  failStream()
  await until(() => calls === 2)
  body.push(payload.subarray(3))
  body.close()
  assert.equal((await finished).status, 'installed')
  assert.equal(service.inflight.size, 0)
})

test('staging directory links reject retry without modifying the external target', async (t) => {
  const { service, dataDir } = await fixture(t, {
    fetchImpl: async () => new Response(payload.subarray(0, 4)),
  })
  await assert.rejects(service.download('sample'), { code: 'size' })
  const staging = await partialDirectory(dataDir)
  const outside = join(dataDir, 'outside')
  await mkdir(outside)
  await writeFile(join(outside, 'model.onnx'), 'keep')
  await rm(join(staging, 'weights'), { recursive: true })
  await symlink(
    outside,
    join(staging, 'weights'),
    process.platform === 'win32' ? 'junction' : 'dir',
  )
  await assert.rejects(service.download('sample'), { code: 'path' })
  assert.equal(await readFile(join(outside, 'model.onnx'), 'utf8'), 'keep')
  assert.equal(service.inflight.size, 0)
})

test('range oversize after a valid prefix never writes beyond the catalog limit', async (t) => {
  let calls = 0
  const { service, dataDir } = await fixture(t, {
    fetchImpl: async () => {
      calls += 1
      if (calls === 1) return new Response(payload.subarray(0, 4))
      return new Response(payload, {
        status: 206,
        headers: { 'content-range': `bytes 4-${payload.length - 1}/${payload.length}` },
      })
    },
  })
  await assert.rejects(service.download('sample'), { code: 'size' })
  await assert.rejects(service.download('sample'), { code: 'size' })
  assert.deepEqual(
    await readFile(join(await partialDirectory(dataDir), 'weights/model.onnx')),
    payload.subarray(0, 4),
  )
})

test('installed model hard links fail full verification and never modify external data', async (t) => {
  const { service, dataDir } = await fixture(t)
  await service.download('sample')
  const installed = await service.modelDirectory('sample')
  const outside = join(dataDir, 'external.bin')
  await link(join(installed, 'weights/model.onnx'), outside)
  await assert.rejects(service.modelDirectory('sample'), /not installed/)
  await assert.rejects(service.download('sample'), { code: 'path' })
  assert.deepEqual(await readFile(outside), payload)
})

test('empty catalog and empty files are valid without network or model execution', async (t) => {
  const { service, create } = await fixture(t, {
    catalog: [],
    fetchImpl: () => assert.fail('unexpected fetch'),
  })
  assert.deepEqual(await service.list(), [])
  const empty = create({ catalog: [{ ...model('empty', Buffer.alloc(0)), kind: 'vad' }] })
  assert.equal((await empty.download('empty')).status, 'installed')
  assert.equal(
    (await readFile(join(await empty.modelDirectory('empty'), 'weights/model.onnx'))).length,
    0,
  )
})

test('archive streams compressed-byte progress, skips ordinary extras and persists cached proof across reload', async (t) => {
  const data = Buffer.from(archiveFixtures.valid, 'base64')
  const body = controlledBody()
  let calls = 0
  const { service, create, dataDir } = await fixture(t, {
    catalog: [archiveModel(data)],
    fetchImpl: async () => {
      calls += 1
      return new Response(body.stream)
    },
  })
  await service.startDownload('sample')
  await until(() => calls === 1)
  body.push(data.subarray(0, 31))
  await until(async () => (await service.status('sample')).downloadedBytes === 31)
  assert.deepEqual(await readdir(await partialDirectory(dataDir)), [])
  body.push(data.subarray(31))
  body.close()
  const result = await service.download('sample')
  assert.equal(result.status, 'installed')
  assert.equal(result.totalBytes, data.length)
  assert.equal(result.downloadedBytes, data.length)
  assert.equal(result.filesBytes, payload.length)
  const directory = await service.modelDirectory('sample')
  assert.deepEqual(await readdir(directory), ['.installation.json', 'weights'])
  assert.deepEqual(await readFile(join(directory, 'weights/model.onnx')), payload)
  assert.deepEqual(await readdir(join(dataDir, 'speech-models')), ['sample'])
  const fresh = create({ fetchImpl: () => assert.fail('no redownload on reload') })
  assert.equal(await fresh.modelDirectory('sample'), directory)
  const reads = await trackContentReads(t, join(directory, 'weights/model.onnx'))
  assert.equal((await fresh.status('sample')).status, 'installed')
  assert.equal((await fresh.list())[0].downloadedBytes, data.length)
  assert.equal(await fresh.modelDirectory('sample'), directory)
  assert.equal(reads.bytes, 0)
  await writeFile(join(directory, 'weights/model.onnx'), Buffer.alloc(payload.length))
  await assert.rejects(fresh.modelDirectory('sample'), { code: 'missing' })
})

test('archive cancellation resumes strict Range after service recreation and trusted redirect', async (t) => {
  const data = Buffer.from(archiveFixtures.valid, 'base64')
  const body = controlledBody()
  let requested = false
  const { service, create } = await fixture(t, {
    catalog: [archiveModel(data)],
    fetchImpl: async () => {
      requested = true
      return new Response(body.stream)
    },
  })
  await service.startDownload('sample')
  await until(() => requested)
  body.push(data.subarray(0, 23))
  await until(async () => (await service.status('sample')).downloadedBytes === 23)
  assert.equal((await service.cancelDownload('sample')).status, 'cancelled')
  assert.equal(body.cancelled, true)
  await service.dispose()
  const fresh = create({
    trustedRedirectHosts: ['release-assets.githubusercontent.com'],
    fetchImpl: async (url, options) => {
      assert.equal(options.headers.Range, 'bytes=23-')
      if (url.includes('cdn.example'))
        return new Response(null, {
          status: 302,
          headers: {
            location: 'https://release-assets.githubusercontent.com/bundle?signature=private',
          },
        })
      return new Response(data.subarray(23), {
        status: 206,
        headers: { 'content-range': `bytes 23-${data.length - 1}/${data.length}` },
      })
    },
  })
  assert.equal((await fresh.download('sample')).status, 'installed')
  assert.equal(JSON.stringify(await fresh.list()).includes('private'), false)
})

for (const [name, code] of [
  ['hash', 'integrity'],
  ['missing', 'integrity'],
  ['duplicate', 'path'],
  ['bomb', 'size'],
  ['totalBomb', 'size'],
  ['absolute', 'path'],
  ['traversal', 'path'],
  ['backslash', 'path'],
  ['windows', 'path'],
  ['prefix', 'path'],
  ['symlink', 'path'],
  ['hardlink', 'path'],
  ['fifo', 'path'],
  ['marker', 'path'],
  ['markerChild', 'path'],
  ['paxEscape', 'path'],
  ['gnuEscape', 'path'],
  ['metadataCount', 'size'],
  ['metadataOversize', 'size'],
  ['paxNegative', 'integrity'],
  ['paxSize', 'path'],
  ['unknownType', 'path'],
  ['tarTruncated', 'size'],
]) {
  test(`archive rejects ${name} and never publishes untrusted entries`, async (t) => {
    const data = Buffer.from(archiveFixtures[name], 'base64')
    let calls = 0
    const { service, dataDir } = await fixture(t, {
      catalog: [archiveModel(data)],
      fetchImpl: async () => {
        calls += 1
        return new Response(data)
      },
    })
    await assert.rejects(service.download('sample'), { code })
    assert.equal(service.inflight.size, 0)
    await assert.rejects(service.modelDirectory('sample'), { code: 'missing' })
    await assert.rejects(readFile(join(await partialDirectory(dataDir), '.installation.json')), {
      code: 'ENOENT',
    })
    await assert.rejects(service.download('sample'), { code })
    assert.equal(calls, 1, 'complete archive retry must not send an out-of-bounds Range')
  })
}

test('archive supports a real GNU long path after prefix stripping', async (t) => {
  const data = Buffer.from(archiveFixtures.gnuLongValid, 'base64')
  const entry = archiveModel(data)
  entry.files[0].path = `${'a'.repeat(110)}/model.onnx`
  const { service } = await fixture(t, {
    catalog: [entry],
    fetchImpl: async () => new Response(data),
  })
  assert.equal((await service.download('sample')).status, 'installed')
  assert.deepEqual(
    await readFile(join(await service.modelDirectory('sample'), entry.files[0].path)),
    payload,
  )
})

test('archive publication rename failure restores the old installation and reuses the verified cache', async (t) => {
  const { service, create, dataDir } = await fixture(t)
  await service.download('sample')
  const directory = await service.modelDirectory('sample')
  const original = await readFile(join(directory, '.installation.json'))
  const data = Buffer.from(archiveFixtures.valid, 'base64')
  let calls = 0
  const newer = create({
    catalog: [archiveModel(data)],
    fetchImpl: async () => {
      calls += 1
      return new Response(data)
    },
  })
  const publish = newer.publish.bind(newer)
  const mock = t.mock.method(newer, 'publish', async (entry, staging) => {
    const moved = `${staging}.moved`
    await rename(staging, moved)
    try {
      await publish(entry, staging)
    } finally {
      await rename(moved, staging)
    }
  })
  await assert.rejects(newer.download('sample'), { code: 'storage' })
  assert.equal(await service.modelDirectory('sample'), directory)
  assert.deepEqual(await readFile(join(directory, '.installation.json')), original)
  assert.equal(
    (await readdir(join(dataDir, 'speech-models'))).some((entry) => entry.endsWith('.previous')),
    false,
  )
  mock.mock.restore()
  assert.equal((await newer.download('sample')).status, 'installed')
  assert.equal(calls, 1)
})

for (const place of ['archive', 'staging']) {
  test(`archive retry refuses hard-linked ${place} without changing the external file`, async (t) => {
    const data = Buffer.from(archiveFixtures.hash, 'base64')
    let calls = 0
    const { service, dataDir } = await fixture(t, {
      catalog: [archiveModel(data)],
      fetchImpl: async () => {
        calls += 1
        return new Response(data)
      },
    })
    await assert.rejects(service.download('sample'), { code: 'integrity' })
    const root = join(dataDir, 'speech-models')
    const path =
      place === 'archive'
        ? join(
            root,
            (await readdir(root)).find((name) => name.endsWith('.tar.bz2')),
          )
        : join(await partialDirectory(dataDir), 'weights/model.onnx')
    const original = await readFile(path)
    const outside = join(dataDir, 'external')
    await link(path, outside)
    await assert.rejects(service.download('sample'), { code: 'path' })
    assert.equal(calls, 1)
    assert.deepEqual(await readFile(outside), original)
  })
}

test('archive honors the effective PAX path rather than its placeholder header', async (t) => {
  const data = Buffer.from(archiveFixtures.paxValid, 'base64')
  const { service } = await fixture(t, {
    catalog: [archiveModel(data)],
    fetchImpl: async () => new Response(data),
  })
  assert.equal((await service.download('sample')).status, 'installed')
  assert.deepEqual(
    await readFile(join(await service.modelDirectory('sample'), 'weights/model.onnx')),
    payload,
  )
})

for (const mode of ['hash', 'download-truncated', 'bzip-truncated']) {
  test(`archive ${mode} fails before publication and closes tasks`, async (t) => {
    const valid = Buffer.from(archiveFixtures.valid, 'base64')
    const data = mode === 'hash' ? Buffer.alloc(valid.length) : valid.subarray(0, valid.length - 12)
    const entry = archiveModel(mode === 'bzip-truncated' ? data : valid)
    const { service, dataDir } = await fixture(t, {
      catalog: [entry],
      fetchImpl: async () => new Response(data),
    })
    await assert.rejects(service.download('sample'), {
      code: mode === 'download-truncated' ? 'size' : 'integrity',
    })
    assert.equal(service.inflight.size, 0)
    await assert.rejects(service.modelDirectory('sample'), { code: 'missing' })
    await assert.rejects(readFile(join(await partialDirectory(dataDir), '.installation.json')), {
      code: 'ENOENT',
    })
    if (mode !== 'bzip-truncated')
      assert.deepEqual(await readdir(await partialDirectory(dataDir)), [])
  })
}

test('archive cancellation waits for an in-flight extraction write and retry rebuilds partial output without fetching', async (t) => {
  const data = Buffer.from(archiveFixtures.valid, 'base64')
  let calls = 0
  const { service, dataDir } = await fixture(t, {
    catalog: [archiveModel(data)],
    fetchImpl: async () => {
      calls += 1
      return new Response(data)
    },
  })
  const probe = await open(join(dataDir, 'probe'), 'w')
  const prototype = Object.getPrototypeOf(probe)
  await probe.close()
  const write = prototype.write
  let entered = false
  let release
  const gate = new Promise((resolve) => {
    release = resolve
  })
  const mocked = t.mock.method(prototype, 'write', async function (...args) {
    if (!entered && service.snapshot(service.getModel('sample')).status === 'verifying') {
      entered = true
      await gate
    }
    return write.apply(this, args)
  })
  await service.startDownload('sample')
  await until(() => entered)
  let settled = false
  const cancelled = service.cancelDownload('sample').then((value) => {
    settled = true
    return value
  })
  await delay(10)
  assert.equal(settled, false)
  assert.equal(service.inflight.size, 1)
  release()
  assert.equal((await cancelled).status, 'cancelled')
  mocked.mock.restore()
  const staging = await partialDirectory(dataDir)
  const moved = `${staging}.moved`
  await rename(staging, moved)
  await rename(moved, staging)
  assert.equal((await service.download('sample')).status, 'installed')
  assert.equal(calls, 1)
  assert.deepEqual(await readdir(join(dataDir, 'speech-models')), ['sample'])
})

test('archive failure preserves the old installation, and a valid archive replaces it atomically', async (t) => {
  const { service, create } = await fixture(t)
  await service.download('sample')
  const directory = await service.modelDirectory('sample')
  const original = await readFile(join(directory, '.installation.json'))
  const bad = Buffer.from(archiveFixtures.hash, 'base64')
  const failed = create({ catalog: [archiveModel(bad)], fetchImpl: async () => new Response(bad) })
  await assert.rejects(failed.download('sample'), { code: 'integrity' })
  assert.equal(await service.modelDirectory('sample'), directory)
  assert.deepEqual(await readFile(join(directory, '.installation.json')), original)
  const data = Buffer.from(archiveFixtures.valid, 'base64')
  const newer = create({ catalog: [archiveModel(data)], fetchImpl: async () => new Response(data) })
  assert.equal((await newer.download('sample')).status, 'installed')
  assert.equal(await newer.modelDirectory('sample'), directory)
  await assert.rejects(service.modelDirectory('sample'), { code: 'missing' })
})

test('archive catalog validates format, bytes, digest, prefix and URLs independently of optional file URLs', async (t) => {
  const { create } = await fixture(t)
  for (const patch of [
    { format: 'tar.gz' },
    { bytes: 0 },
    { bytes: NaN },
    { sha256: 'bad' },
    { stripPrefix: '../' },
    { stripPrefix: '/bundle/' },
    { stripPrefix: 'bundle' },
    { stripPrefix: 'NUL/' },
    { stripPrefix: 'a\\b/' },
    { urls: [] },
    { urls: ['http://cdn.example/a'] },
  ]) {
    const entry = archiveModel()
    Object.assign(entry.archive, patch)
    assert.throws(() => create({ catalog: [entry] }), { code: 'catalog' })
  }
  const entry = archiveModel()
  const first = create({ catalog: [entry] })
  const fingerprint = first.getModel('sample').fingerprint
  entry.archive.sha256 = '0'.repeat(64)
  assert.notEqual(create({ catalog: [entry] }).getModel('sample').fingerprint, fingerprint)
  assert.notEqual(first.getModel('sample').archive.sha256, entry.archive.sha256)
})
