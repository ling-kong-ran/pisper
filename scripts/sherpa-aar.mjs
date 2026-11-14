// 改写官方 sherpa-onnx AAR，让 Pisper 自己决定何时、从哪里加载语音原生库。
// sherpa 的 Kotlin 类在静态初始化里调用 System.loadLibrary("sherpa-onnx-jni")，只会去 APK 的
// nativeLibraryDir 找；Android 11 以前找不到就直接抛错。原生库改为按需下载后，这一句必须去掉：
// 把 `ldc "sherpa-onnx-jni"; invokestatic System.loadLibrary` 换成等长的 nop，栈平衡和跳转偏移
// 都不变，其余字节码原样保留。加载改由 SpeechNativeRuntime.kt 统一完成。
// 同时只留下 arm64 的 JNI 库和它依赖的 onnxruntime：App 只构建 arm64，C/C++ 接口库没人加载。
import { unzipSync, zipSync } from 'fflate'

export const SHERPA_JNI_LIBRARY = 'sherpa-onnx-jni'
export const SHERPA_NATIVE_LIBRARIES = Object.freeze(['libonnxruntime.so', 'libsherpa-onnx-jni.so'])
const NATIVE_PREFIX = 'jni/arm64-v8a/'

const textDecoder = new TextDecoder()

function readConstantPool(bytes) {
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength)
  if (view.getUint32(0) !== 0xcafebabe) throw new Error('Not a Java class file')
  const count = view.getUint16(8)
  const entries = new Array(count)
  let offset = 10
  for (let index = 1; index < count; index += 1) {
    const tag = bytes[offset]
    switch (tag) {
      case 1: {
        const length = view.getUint16(offset + 1)
        entries[index] = {
          tag,
          value: textDecoder.decode(bytes.subarray(offset + 3, offset + 3 + length)),
        }
        offset += 3 + length
        break
      }
      case 7:
      case 8:
      case 16:
      case 19:
      case 20:
        entries[index] = { tag, ref: view.getUint16(offset + 1) }
        offset += 3
        break
      case 15:
        offset += 4
        break
      case 3:
      case 4:
      case 17:
      case 18:
        offset += 5
        break
      case 9:
      case 10:
      case 11:
      case 12:
        entries[index] = { tag, ref: view.getUint16(offset + 1), ref2: view.getUint16(offset + 3) }
        offset += 5
        break
      case 5:
      case 6:
        // long/double 占两个常量池槽位。
        offset += 9
        index += 1
        break
      default:
        throw new Error(`Unknown constant pool tag ${tag}`)
    }
  }
  return { view, entries, end: offset }
}

function loadLibraryReferences(entries) {
  const utf8 = (index) => (entries[index]?.tag === 1 ? entries[index].value : null)
  const methods = []
  const strings = []
  entries.forEach((entry, index) => {
    if (entry?.tag === 8 && utf8(entry.ref) === SHERPA_JNI_LIBRARY) strings.push(index)
    if (entry?.tag !== 10) return
    const owner = entries[entry.ref]
    const signature = entries[entry.ref2]
    if (
      owner?.tag === 7 &&
      utf8(owner.ref) === 'java/lang/System' &&
      signature?.tag === 12 &&
      utf8(signature.ref) === 'loadLibrary' &&
      utf8(signature.ref2) === '(Ljava/lang/String;)V'
    )
      methods.push(index)
  })
  return { methods, strings }
}

// 遍历字段、方法和属性表，只在 Code 属性的字节码里查找要替换的指令序列。
function codeRanges(bytes, view, entries, start) {
  const ranges = []
  let offset = start + 6
  offset += 2 + view.getUint16(offset) * 2
  const skipAttributes = (cursor, onAttribute) => {
    const attributes = view.getUint16(cursor)
    cursor += 2
    for (let index = 0; index < attributes; index += 1) {
      const name = entries[view.getUint16(cursor)]?.value
      const length = view.getUint32(cursor + 2)
      onAttribute?.(name, cursor + 6, length)
      cursor += 6 + length
    }
    return cursor
  }
  // 先字段表、再方法表，两张表结构相同。
  for (let table = 0; table < 2; table += 1) {
    const members = view.getUint16(offset)
    offset += 2
    for (let index = 0; index < members; index += 1) {
      offset = skipAttributes(offset + 6, (name, body) => {
        if (name !== 'Code') return
        const length = view.getUint32(body + 4)
        ranges.push([body + 8, body + 8 + length])
      })
    }
  }
  skipAttributes(offset)
  return ranges
}

// 补丁后仍有任何 System.loadLibrary 调用（例如换了库名），说明上游改了写法，构建应失败。
function callsLoadLibrary(classBytes) {
  const { view, entries, end } = readConstantPool(classBytes)
  const { methods } = loadLibraryReferences(entries)
  if (!methods.length) return false
  return codeRanges(classBytes, view, entries, end).some(([from, to]) => {
    for (let cursor = from; cursor + 2 < to; cursor += 1) {
      if (
        classBytes[cursor] === 0xb8 &&
        methods.includes((classBytes[cursor + 1] << 8) | classBytes[cursor + 2])
      )
        return true
    }
    return false
  })
}

export function stripLoadLibrary(classBytes) {
  const bytes = Uint8Array.from(classBytes)
  const { view, entries, end } = readConstantPool(bytes)
  const { methods, strings } = loadLibraryReferences(entries)
  if (!methods.length || !strings.length) return { bytes, patched: 0 }
  let patched = 0
  for (const [from, to] of codeRanges(bytes, view, entries, end)) {
    for (let cursor = from; cursor < to; cursor += 1) {
      for (const string of strings) {
        const load = string <= 0xff ? [0x12, string] : [0x13, string >> 8, string & 0xff]
        for (const method of methods) {
          const sequence = [...load, 0xb8, method >> 8, method & 0xff]
          if (cursor + sequence.length > to) continue
          if (sequence.every((value, index) => bytes[cursor + index] === value)) {
            bytes.fill(0x00, cursor, cursor + sequence.length)
            patched += 1
          }
        }
      }
    }
  }
  return { bytes, patched }
}

export function patchSherpaAar(aarBytes) {
  const archive = unzipSync(aarBytes)
  const classesJar = archive['classes.jar']
  if (!classesJar) throw new Error('sherpa AAR has no classes.jar')
  const classes = unzipSync(classesJar)
  let patchedClasses = 0
  for (const [name, contents] of Object.entries(classes)) {
    if (!name.startsWith('com/k2fsa/sherpa/onnx/') || !name.endsWith('.class')) continue
    const { bytes, patched } = stripLoadLibrary(contents)
    if (!patched) continue
    classes[name] = bytes
    patchedClasses += 1
  }
  if (!patchedClasses) throw new Error('sherpa AAR no longer loads its JNI library statically')
  const remaining = Object.entries(classes).find(
    ([name, contents]) => name.endsWith('.class') && callsLoadLibrary(contents),
  )
  if (remaining) throw new Error(`sherpa AAR still loads native code in ${remaining[0]}`)

  const libraries = {}
  const output = {}
  for (const [name, contents] of Object.entries(archive)) {
    if (name === 'classes.jar') continue
    if (name.startsWith('jni/')) {
      const library = name.slice(NATIVE_PREFIX.length)
      if (name.startsWith(NATIVE_PREFIX) && SHERPA_NATIVE_LIBRARIES.includes(library)) {
        libraries[library] = contents
        output[name] = [contents, { level: 0 }]
      }
      continue
    }
    output[name] = contents
  }
  for (const library of SHERPA_NATIVE_LIBRARIES) {
    if (!libraries[library]) throw new Error(`sherpa AAR is missing arm64 ${library}`)
  }
  output['classes.jar'] = zipSync(classes)
  return { aar: zipSync(output), libraries, patchedClasses }
}
