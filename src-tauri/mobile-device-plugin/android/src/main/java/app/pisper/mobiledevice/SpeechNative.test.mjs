import assert from 'node:assert/strict'
import { createHash } from 'node:crypto'
import { test } from 'node:test'
import { mkdtemp, mkdir, readFile, readdir, rm, writeFile } from 'node:fs/promises'
import { homedir, tmpdir } from 'node:os'
import { delimiter, dirname, join, resolve } from 'node:path'
import { spawnSync } from 'node:child_process'
import { fileURLToPath } from 'node:url'

const sourceDir = dirname(fileURLToPath(import.meta.url))
const cache = join(homedir(), '.gradle/caches/modules-2/files-2.1')

async function jar(group, artifact) {
  const root = join(cache, group, artifact)
  const versions = (await readdir(root)).sort((a, b) =>
    b.localeCompare(a, undefined, { numeric: true }),
  )
  for (const version of versions) {
    for (const hash of await readdir(join(root, version))) {
      const directory = join(root, version, hash)
      for (const name of await readdir(directory)) {
        if (name.endsWith('.jar') && !name.endsWith('-sources.jar')) return join(directory, name)
      }
    }
  }
  throw new Error(`Missing cached Kotlin dependency ${group}:${artifact}`)
}

async function archiveLibraries(dir) {
  const dependencies = [
    [
      'org.apache.commons',
      'commons-compress',
      '1.28.0',
      'e1522945218456f3649a39bc4afd70ce4bd466221519dba7d378f2141a4642ca',
    ],
    [
      'commons-io',
      'commons-io',
      '2.20.0',
      'df90bba0fe3cb586b7f164e78fe8f8f4da3f2dd5c27fa645f888100ccc25dd72',
    ],
    [
      'org.apache.commons',
      'commons-lang3',
      '3.18.0',
      '4eeeae8d20c078abb64b015ec158add383ac581571cddc45c68f0c9ae0230720',
    ],
    [
      'commons-codec',
      'commons-codec',
      '1.19.0',
      '5c3881e4f556855e9c532927ee0c9dfde94cc66760d5805c031a59887070af5f',
    ],
  ]
  return Promise.all(
    dependencies.map(async ([group, artifact, version, digest]) => {
      const name = `${artifact}-${version}.jar`
      const cachedRoot = join(cache, group, artifact, version)
      for (const hash of await readdir(cachedRoot).catch(() => [])) {
        const path = join(cachedRoot, hash, name)
        const bytes = await readFile(path).catch(() => null)
        if (bytes && createHash('sha256').update(bytes).digest('hex') === digest) return path
      }
      // 只在临时目录取固定摘要的测试依赖，不改项目锁文件或 Gradle 缓存。
      const url = `https://repo.maven.apache.org/maven2/${group.replaceAll('.', '/')}/${artifact}/${version}/${name}`
      const response = await fetch(url, { signal: AbortSignal.timeout(30_000) })
      assert.equal(response.status, 200)
      const bytes = Buffer.from(await response.arrayBuffer())
      assert.equal(createHash('sha256').update(bytes).digest('hex'), digest)
      const path = join(dir, name)
      await writeFile(path, bytes)
      return path
    }),
  )
}

function execute(command, args) {
  const result = spawnSync(command, args, {
    encoding: 'utf8',
    timeout: 90_000,
    maxBuffer: 4 * 1024 * 1024,
  })
  assert.equal(result.status, 0, `${result.error ?? ''}\n${result.stdout}\n${result.stderr}`)
  return result.stdout
}

const harness = String.raw`
package app.pisper.mobiledevice

import java.io.ByteArrayInputStream
import java.io.File
import java.io.FileOutputStream
import org.apache.commons.compress.archivers.tar.TarArchiveEntry
import org.apache.commons.compress.archivers.tar.TarArchiveOutputStream
import org.apache.commons.compress.archivers.tar.TarConstants
import org.apache.commons.compress.compressors.bzip2.BZip2CompressorOutputStream
import java.net.HttpURLConnection
import java.net.URL
import java.nio.ByteBuffer
import java.nio.ByteOrder
import java.security.MessageDigest

private fun expectFailure(block: () -> Unit) {
    var failed = false
    try { block() } catch (_: Exception) { failed = true }
    check(failed) { "Expected failure" }
}
private fun sha(value: ByteArray): String = MessageDigest.getInstance("SHA-256").digest(value)
    .joinToString("") { "%02x".format(it) }

private class Response(
    url: URL, val status: Int, val data: ByteArray, val headers: Map<String, String> = emptyMap(),
) : HttpURLConnection(url) {
    override fun connect() {}
    override fun disconnect() {}
    override fun usingProxy() = false
    override fun getResponseCode() = status
    override fun getHeaderField(name: String) = headers[name]
    override fun getInputStream() = ByteArrayInputStream(data)
}

private fun makeArchive(file: File, entries: List<Pair<TarArchiveEntry, ByteArray>>) {
    FileOutputStream(file).use { output ->
        BZip2CompressorOutputStream(output).use { bzip ->
            TarArchiveOutputStream(bzip).use { tar ->
                for ((entry, bytes) in entries) {
                    entry.size = bytes.size.toLong()
                    tar.putArchiveEntry(entry)
                    tar.write(bytes)
                    tar.closeArchiveEntry()
                }
                tar.finish()
            }
        }
    }
}

fun main(args: Array<String>) {
    val dir = File(args[0]).canonicalFile
    val data = "verified speech resource".toByteArray()
    val spec = SpeechDownloadFile("data !/model.onnx", data.size.toLong(), sha(data), listOf("https://hf-mirror.com/model"))
    check(SpeechModelFiles.relativePath("espeak-ng-data/lang/a ! b") == "espeak-ng-data/lang/a ! b")
    for (path in listOf("", "..", "a/../b", "/root", "a//b", "a/./b", "C:/a", "a\\b", "a\u0000b")) {
        expectFailure { SpeechModelFiles.relativePath(path) }
    }
    for (url in listOf("http://hf-mirror.com/a", "https://127.0.0.1/a", "https://hf-mirror.com.evil/a", "https://u:p@hf-mirror.com/a", "https://hf-mirror.com:8443/a", "https://hf-mirror.com/a#b")) {
        expectFailure { SpeechModelFiles.trustedUrl(url) }
    }
    val file = SpeechModelFiles.child(dir, spec.path)
    val progress = mutableListOf<Long>()
    SpeechModelFiles.download(file, spec, SpeechDownloadCancellation(), { progress += it }) {
        Response(it, 200, data, mapOf("Content-Length" to data.size.toString()))
    }
    check(file.readBytes().contentEquals(data) && progress.last() == data.size.toLong())
    var cacheConnections = 0
    SpeechModelFiles.download(file, spec, SpeechDownloadCancellation(), {}) {
        cacheConnections++; Response(it, 200, data)
    }
    check(cacheConnections == 0)

    file.writeBytes(data.copyOfRange(0, 7))
    val connections = mutableListOf<Response>()
    SpeechModelFiles.download(file, spec, SpeechDownloadCancellation(), {}) {
        Response(it, 206, data.copyOfRange(7, data.size), mapOf("Content-Range" to ("bytes 7-" + (data.size - 1) + "/" + data.size)))
            .also { response -> connections += response }
    }
    check(connections.single().getRequestProperty("Range") == "bytes=7-")
    check(file.readBytes().contentEquals(data))

    file.writeBytes(data.copyOfRange(0, 5))
    SpeechModelFiles.download(file, spec, SpeechDownloadCancellation(), {}) { Response(it, 200, data) }
    check(file.length() == data.size.toLong())
    file.writeBytes(byteArrayOf(0))
    expectFailure {
        SpeechModelFiles.download(file, spec, SpeechDownloadCancellation(), {}) {
            Response(it, 206, data, mapOf("Content-Range" to ("bytes 0-" + (data.size - 1) + "/" + data.size)))
        }
    }
    check(file.length() == 1L)

    file.delete()
    var redirects = 0
    SpeechModelFiles.download(file, spec, SpeechDownloadCancellation(), {}) {
        redirects++
        if (it.host == "hf-mirror.com") Response(it, 302, byteArrayOf(), mapOf("Location" to "https://cas-bridge.xethub.hf.co/blob?signature=transient"))
        else Response(it, 200, data)
    }
    check(redirects == 2 && file.readBytes().contentEquals(data))
    file.delete()
    var privateConnections = 0
    expectFailure {
        SpeechModelFiles.download(file, spec, SpeechDownloadCancellation(), {}) {
            if (it.host == "127.0.0.1") privateConnections++
            Response(it, 302, byteArrayOf(), mapOf("Location" to "https://127.0.0.1/private"))
        }
    }
    check(privateConnections == 0 && !file.exists())

    val cancelled = SpeechDownloadCancellation()
    expectFailure {
        SpeechModelFiles.download(file, spec, cancelled, { if (it > 0) cancelled.cancelled.set(true) }) {
            Response(it, 200, data)
        }
    }
    check(cancelled.cancelled.get())
    SpeechModelFiles.download(file, spec, SpeechDownloadCancellation(), {}) { Response(it, 200, data) }
    check(SpeechModelFiles.verify(file, spec))
    file.delete()
    expectFailure {
        SpeechModelFiles.download(file, spec, SpeechDownloadCancellation(), {}) {
            Response(it, 200, ByteArray(data.size) { 0 })
        }
    }
    check(!file.exists())
    expectFailure {
        SpeechModelFiles.download(file, spec, SpeechDownloadCancellation(), {}) {
            Response(it, 200, ByteArray(data.size + 1))
        }
    }
    check(file.length() <= data.size)

    val big = ByteArray(180_000) { (it % 127).toByte() }
    val bigFile = File(dir, "resume.bin")
    val bigSpec = SpeechDownloadFile("resume.bin", big.size.toLong(), sha(big), spec.urls)
    val midCancel = SpeechDownloadCancellation()
    expectFailure {
        SpeechModelFiles.download(bigFile, bigSpec, midCancel, { if (it > 0) midCancel.cancelled.set(true) }) {
            Response(it, 200, big)
        }
    }
    val retained = bigFile.length().toInt()
    check(retained > 0 && retained < big.size)
    SpeechModelFiles.download(bigFile, bigSpec, SpeechDownloadCancellation(), {}) {
        Response(it, 206, big.copyOfRange(retained, big.size),
            mapOf("Content-Range" to ("bytes " + retained + "-" + (big.size - 1) + "/" + big.size)))
    }
    check(bigFile.readBytes().contentEquals(big))

    val staged = File(dir, "staged").also { it.mkdirs() }
    val installed = File(dir, "installed").also { it.mkdirs() }
    val retired = File(dir, "retired")
    File(staged, "version").writeText("new")
    File(installed, "version").writeText("old")
    expectFailure {
        SpeechModelFiles.publish(staged, installed, retired, {}) { from, to ->
            if (from == staged) false else from.renameTo(to)
        }
    }
    check(File(installed, "version").readText() == "old" && staged.isDirectory && !retired.exists())
    expectFailure { SpeechModelFiles.publish(staged, installed, retired, { throw SpeechCancelled() }) }
    check(File(installed, "version").readText() == "old")
    SpeechModelFiles.publish(staged, installed, retired, {})
    check(File(installed, "version").readText() == "new" && File(retired, "version").readText() == "old")
    SpeechModelFiles.deleteTree(retired)
    check(!retired.exists())

    val archive = File(dir, "model.tar.bz2")
    val modelEntry = TarArchiveEntry("pkg/data !/model.onnx") to data
    val rootEntry = TarArchiveEntry("pkg/") to byteArrayOf()
    var extraction = 0
    fun extract(entries: List<Pair<TarArchiveEntry, ByteArray>>, maximum: Long = 1_000_000, entryLimit: Int = 100): File {
        makeArchive(archive, entries)
        val target = File(dir, "extracted-" + extraction++).also { it.mkdirs() }
        SpeechModelArchive.extract(archive, target, "pkg/", listOf(spec), {}, entryLimit, maximum)
        return target
    }
    val extracted = extract(listOf(rootEntry, modelEntry,
        TarArchiveEntry("pkg/README.md") to "readme".toByteArray(),
        TarArchiveEntry("pkg/test_wavs/sample.wav") to ByteArray(512)))
    check(File(extracted, spec.path).readBytes().contentEquals(data))
    check(!File(extracted, "README.md").exists() && !File(extracted, "test_wavs").exists())
    expectFailure { extract(listOf(rootEntry, modelEntry, TarArchiveEntry("pkg/../outside") to data)) }
    expectFailure { extract(listOf(rootEntry, modelEntry, TarArchiveEntry("/outside", true) to data)) }
    expectFailure { extract(listOf(rootEntry, modelEntry, TarArchiveEntry("other/file") to data)) }
    expectFailure { extract(listOf(rootEntry, modelEntry, modelEntry)) }
    expectFailure { extract(listOf(rootEntry)) }
    expectFailure { extract(listOf(rootEntry, TarArchiveEntry("pkg/data !/model.onnx") to ByteArray(data.size))) }
    expectFailure { extract(listOf(rootEntry, modelEntry), entryLimit = 1) }
    expectFailure { extract(listOf(rootEntry, modelEntry, TarArchiveEntry("pkg/skipped.bin") to ByteArray(8192)), maximum = 4096) }
    for (type in listOf(TarConstants.LF_SYMLINK, TarConstants.LF_LINK, TarConstants.LF_FIFO)) {
        val link = TarArchiveEntry("pkg/link", type).also { it.linkName = "../outside" }
        expectFailure { extract(listOf(rootEntry, modelEntry, link to byteArrayOf())) }
    }
    makeArchive(archive, listOf(rootEntry, modelEntry))
    expectFailure {
        SpeechModelArchive.extract(archive, File(dir, "cancel-extract"), "pkg/", listOf(spec), { throw SpeechCancelled() })
    }

    val id = "00000000-0000-4000-8000-000000000001"
    val other = "00000000-0000-4000-8000-000000000002"
    expectFailure { SpeechRequests.validateId("1-1-1-1-1") }
    var clock = 0L
    val requests = SpeechRequests { clock }
    requests.cancel(id)
    expectFailure { requests.begin(id) }
    val running = requests.begin(other)
    expectFailure { requests.begin(other) }
    requests.cancel(id)
    running.check()
    requests.pause()
    expectFailure { running.check() }
    expectFailure { requests.begin("00000000-0000-4000-8000-000000000003") }
    requests.resume()
    expectFailure { requests.begin(id) }
    requests.finish(running)
    clock += 11 * 60_000
    requests.begin(id).check()

    val wave = File(dir, "test.wav")
    SpeechWave.write(wave, floatArrayOf(0f, -1f, 1f, 2f), 24_000) {}
    val bytes = wave.readBytes()
    check(bytes.size == 52 && String(bytes.copyOfRange(0, 4)) == "RIFF")
    val header = ByteBuffer.wrap(bytes).order(ByteOrder.LITTLE_ENDIAN)
    check(header.getInt(24) == 24_000 && header.getShort(34).toInt() == 16 && header.getInt(40) == 8)
    check(header.getShort(44).toInt() == 0 && header.getShort(46).toInt() == -32767 && header.getShort(50).toInt() == 32767)
    expectFailure { SpeechWave.write(wave, floatArrayOf(Float.NaN), 24_000) {} }
    expectFailure { SpeechWave.write(wave, FloatArray(8_000 * 45 + 1), 8_000) {} }
    expectFailure { SpeechWave.write(wave, floatArrayOf(0f), 192_000) {} }
    expectFailure { SpeechWave.write(wave, floatArrayOf(0f), 24_000) { throw SpeechCancelled() } }
    println("PASS: path/URL policy, streaming, resume, redirect, integrity, cancellation isolation, lifecycle, WAV limits")
}
`

test(
  'Android speech helper behavior and real AAR service signatures',
  { timeout: 120_000 },
  async () => {
    const dir = await mkdtemp(join(tmpdir(), 'pisper-speech-native-'))
    try {
      const toolchain = join(homedir(), '.pisper-android')
      const jdk =
        process.env.JAVA_HOME ||
        join(
          toolchain,
          (await readdir(toolchain)).find((name) => name.startsWith('jdk-')),
        )
      const java = join(jdk, 'bin', process.platform === 'win32' ? 'java.exe' : 'java')
      const jars = await Promise.all([
        jar('org.jetbrains.kotlin', 'kotlin-compiler-embeddable'),
        jar('org.jetbrains.kotlin', 'kotlin-stdlib'),
        jar('org.jetbrains.kotlin', 'kotlin-reflect'),
        jar('org.jetbrains.kotlin', 'kotlin-script-runtime'),
        jar('org.jetbrains.intellij.deps', 'trove4j'),
        jar('org.jetbrains', 'annotations'),
        jar('org.jetbrains.kotlinx', 'kotlinx-coroutines-core-jvm'),
      ])
      const commons = await archiveLibraries(dir)
      const android = join(
        process.env.ANDROID_HOME || join(toolchain, 'sdk'),
        'platforms/android-36/android.jar',
      )
      const aar = resolve(sourceDir, '../../../../../../libs/sherpa-onnx.aar')
      const extracted = join(dir, 'aar')
      await mkdir(extracted)
      const jarTool = join(jdk, 'bin', process.platform === 'win32' ? 'jar.exe' : 'jar')
      const extract = spawnSync(jarTool, ['xf', aar, 'classes.jar'], {
        cwd: extracted,
        encoding: 'utf8',
      })
      assert.equal(extract.status, 0, extract.stderr)
      const stub = join(dir, 'JSObject.kt')
      await writeFile(stub, 'package app.tauri.plugin\nclass JSObject : org.json.JSONObject()\n')
      const fixture = join(dir, 'SpeechNativeHarness.kt')
      await writeFile(fixture, harness)
      const classes = join(dir, 'classes')
      const libraryPath = [
        ...jars.slice(1),
        ...commons,
        android,
        join(extracted, 'classes.jar'),
      ].join(delimiter)
      execute(java, [
        '-cp',
        jars.join(delimiter),
        'org.jetbrains.kotlin.cli.jvm.K2JVMCompiler',
        '-no-stdlib',
        '-no-reflect',
        '-jvm-target',
        '1.8',
        '-classpath',
        libraryPath,
        '-d',
        classes,
        ...[
          'SpeechModelFiles.kt',
          'SpeechModelArchive.kt',
          'SpeechAudioState.kt',
          'SpeechModelStore.kt',
          'SpeechAudioService.kt',
        ].map((name) => join(sourceDir, name)),
        stub,
        fixture,
      ])
      const output = execute(java, [
        '-cp',
        [classes, ...jars.slice(1), ...commons].join(delimiter),
        'app.pisper.mobiledevice.SpeechNativeHarnessKt',
        dir,
      ])
      assert.match(output, /PASS: path\/URL policy/)
      const bytecode = await readFile(join(extracted, 'classes.jar'))
      assert.ok(bytecode.length > 0)
    } finally {
      await rm(dir, { recursive: true, force: true })
    }
  },
)
