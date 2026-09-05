import assert from 'node:assert/strict'
import { spawnSync } from 'node:child_process'
import { existsSync, mkdtempSync, readdirSync, rmSync, writeFileSync } from 'node:fs'
import { homedir, tmpdir } from 'node:os'
import { delimiter, dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import test from 'node:test'

const sourceDir = dirname(fileURLToPath(import.meta.url))
const cache = join(
  process.env.GRADLE_USER_HOME || join(homedir(), '.gradle'),
  'caches/modules-2/files-2.1',
)
const version = process.env.KOTLIN_VERSION || '1.9.25'

function jar(group, artifact, release) {
  const base = join(cache, group, artifact, release)
  for (const hash of readdirSync(base)) {
    const path = join(base, hash, `${artifact}-${release}.jar`)
    if (existsSync(path)) return path
  }
  throw new Error(`Missing cached Kotlin dependency: ${artifact}-${release}; compile Android first`)
}

// 复用 Android 构建缓存的编译器，不新增 Gradle 依赖，也不把测试代码编入 APK。
test('workspace importer enforces safe copying and rolls back failures', () => {
  let javaHome = process.env.JAVA_HOME
  const portable = join(homedir(), '.pisper-android')
  if (!javaHome && existsSync(portable)) {
    const jdk = readdirSync(portable).find((entry) => entry.startsWith('jdk-'))
    if (jdk) javaHome = join(portable, jdk)
  }
  const java = javaHome
    ? join(javaHome, 'bin', process.platform === 'win32' ? 'java.exe' : 'java')
    : 'java'
  const stdlib = jar('org.jetbrains.kotlin', 'kotlin-stdlib', version)
  const compiler = [
    jar('org.jetbrains.kotlin', 'kotlin-compiler-embeddable', version),
    stdlib,
    jar('org.jetbrains.kotlin', 'kotlin-script-runtime', version),
    jar('org.jetbrains.kotlin', 'kotlin-reflect', '1.6.10'),
    jar('org.jetbrains.intellij.deps', 'trove4j', '1.0.20200330'),
    jar('org.jetbrains', 'annotations', '13.0'),
  ].join(delimiter)
  const temporary = mkdtempSync(join(tmpdir(), 'pisper-workspace-import-test-'))
  try {
    const suite = join(temporary, 'WorkspaceDirectoryImporterTest.kt')
    writeFileSync(suite, kotlinTests)
    const classes = join(temporary, 'classes')
    const compile = spawnSync(
      java,
      [
        '-cp',
        compiler,
        'org.jetbrains.kotlin.cli.jvm.K2JVMCompiler',
        '-no-stdlib',
        '-no-reflect',
        '-classpath',
        stdlib,
        '-jvm-target',
        '1.8',
        '-d',
        classes,
        join(sourceDir, 'WorkspaceDirectoryImporter.kt'),
        suite,
      ],
      { encoding: 'utf8', timeout: 60_000 },
    )
    assert.equal(compile.status, 0, `${compile.error || ''}\n${compile.stdout}\n${compile.stderr}`)
    const run = spawnSync(
      java,
      [
        '-cp',
        [classes, stdlib].join(delimiter),
        'app.pisper.mobiledevice.WorkspaceDirectoryImporterTestKt',
      ],
      { encoding: 'utf8', timeout: 60_000 },
    )
    assert.equal(run.status, 0, `${run.error || ''}\n${run.stdout}\n${run.stderr}`)
    assert.match(run.stdout, /PASS: all workspace import helper checks/)
    console.log(run.stdout.trim())
  } finally {
    rmSync(temporary, { recursive: true, force: true })
  }
})

const kotlinTests = String.raw`
package app.pisper.mobiledevice

import java.io.File
import java.io.IOException
import java.io.InputStream
import java.io.OutputStream
import java.nio.file.Files
import java.nio.file.StandardOpenOption.CREATE_NEW
import java.nio.file.StandardOpenOption.WRITE

private val root = WorkspaceDocument("root", "project", true)
private val file = WorkspaceDocument("file", "hello.txt", false)
private val directory = WorkspaceDocument("directory", "src", true)
private val create: (File) -> OutputStream = { Files.newOutputStream(it.toPath(), CREATE_NEW, WRITE) }

private open class Source(
    private val selected: WorkspaceDocument = root,
    private val tree: Map<String, List<WorkspaceDocument>> = mapOf("root" to listOf(file)),
) : WorkspaceDocumentSource {
    override fun root() = selected
    override fun children(document: WorkspaceDocument, visit: (WorkspaceDocument) -> Unit) {
        tree[document.id].orEmpty().forEach(visit)
    }
    override fun open(document: WorkspaceDocument): InputStream = "abc".byteInputStream()
}

private fun expectFailure(block: () -> Unit): Throwable {
    try { block() } catch (error: Exception) { return error }
    error("Expected import failure")
}

fun main() {
    val data = Files.createTempDirectory("pisper-import-files-").toFile().canonicalFile
    val destination = File(data, "local-runtime-data/workspace").path
    val importer = WorkspaceDirectoryImporter(create)
    try {
        val imported = importer.importDirectory(data, destination, Source(tree = mapOf(
            "root" to listOf(directory), "directory" to listOf(file),
        )))
        check(imported.name == "project" && imported.isAbsolute)
        check(imported.parentFile.name.startsWith("import-"))
        check(imported.parentFile.parentFile.path == destination)
        check(File(imported, "src/hello.txt").readText() == "abc")
        File(imported, "src/hello.txt").writeText("local edit")
        check(Source().open(file).reader().readText() == "abc")
        val again = importer.importDirectory(data, destination, Source())
        check(imported != again && File(imported, "src/hello.txt").readText() == "local edit")
        val existing = File(destination).list()!!.toSet()
        fun fails(source: WorkspaceDocumentSource, copy: WorkspaceDirectoryImporter = importer) {
            expectFailure { copy.importDirectory(data, destination, source) }
            check(File(destination).list()!!.toSet() == existing) { "Staging directory leaked" }
            check(File(imported, "src/hello.txt").readText() == "local edit")
        }
        for (name in listOf("", " ", ".", "..", "../escape", "/absolute", "a/b", "a\\b", "a\u0000b",
            "a\nb", "a\u007fb", "a\u0085b", "a\u202eb", "a\ud800b", "x".repeat(256))) {
            fails(Source(selected = root.copy(name = name)))
            fails(Source(tree = mapOf("root" to listOf(file.copy(name = name)))))
        }
        fails(Source(selected = file))
        fails(Source(tree = mapOf("root" to listOf(file, file.copy(id = "other")))))
        fails(Source(tree = mapOf("root" to listOf(file, file.copy(name = "other.txt")))))
        fails(Source(tree = mapOf("root" to listOf(directory), "directory" to listOf(root))))
        fails(Source(tree = mapOf("root" to listOf(file.copy(id = "")))))
        fails(Source(), WorkspaceDirectoryImporter(create, maxBytes = 2))
        fails(Source(tree = mapOf("root" to listOf(file, file.copy(id = "two", name = "two.txt")))),
            WorkspaceDirectoryImporter(create, maxBytes = 5))
        fails(Source(), WorkspaceDirectoryImporter(create, maxEntries = 1))
        fails(Source(), WorkspaceDirectoryImporter(create, maxDepth = 1))
        fails(object : Source() {
            override fun children(document: WorkspaceDocument, visit: (WorkspaceDocument) -> Unit) {
                visit(file)
                throw IOException("provider lost access")
            }
        })
        fails(object : Source() {
            override fun open(document: WorkspaceDocument): InputStream = object : InputStream() {
                override fun read(): Int = throw IOException("provider read failed")
            }
        })
        fails(Source(), WorkspaceDirectoryImporter({ throw IOException("disk full") }))
        fails(Source(), WorkspaceDirectoryImporter({ target ->
            target.writeText("do not overwrite")
            try { create(target) } finally { check(target.readText() == "do not overwrite") }
        }))
        fails(Source(), WorkspaceDirectoryImporter({ target ->
            val output = create(target)
            object : OutputStream() {
                override fun write(value: Int) = output.write(value)
                override fun close() { output.close(); throw IOException("close failed") }
            }
        }))
        for (path in listOf(data.path, File(data, "files/workspace").path,
            File(data, "local-runtime-data/workspace/../other").path, "relative", data.parentFile.path)) {
            expectFailure { importer.importDirectory(data, path, Source()) }
        }
        val exact = WorkspaceDirectoryImporter(create, maxBytes = 3, maxEntries = 2, maxDepth = 2)
            .importDirectory(data, destination, Source())
        check(File(exact, "hello.txt").readText() == "abc")
        val unicode = importer.importDirectory(data, destination,
            Source(selected = root.copy(name = "\u5de5\u4f5c\u533a\ud83d\ude80")))
        check(unicode.isDirectory)
        val empty = WorkspaceDirectoryImporter(create, maxBytes = 0, maxEntries = 1, maxDepth = 1)
            .importDirectory(data, destination, Source(tree = emptyMap()))
        check(empty.list()!!.isEmpty())
        expectFailure { WorkspaceDirectoryImporter(create, maxBytes = 256L * 1024 * 1024 + 1) }
        expectFailure { WorkspaceDirectoryImporter(create, maxEntries = 10_001) }
        expectFailure { WorkspaceDirectoryImporter(create, maxDepth = 65) }
        println("PASS: nested import, unique paths, original unchanged, unsafe names, duplicate paths/IDs, cycles")
        println("PASS: byte/entry/depth boundaries, provider/read/write/close failures, staging cleanup, private root")
        println("PASS: all workspace import helper checks")
    } finally {
        data.deleteRecursively()
    }
}
`
