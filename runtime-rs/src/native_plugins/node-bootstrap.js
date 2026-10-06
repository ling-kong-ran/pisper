// Browser-independent QuickJS bindings; native operations execute in the Rust worker.
;(() => {
  const bridge = globalThis.__pisperNative
  delete globalThis.__pisperNative
  function native(op, ...args) {
    const response = JSON.parse(bridge(op, JSON.stringify(args)))
    if (!response.ok) {
      const error = new Error(response.message)
      const code = /^([A-Z][A-Z_]+):/.exec(response.message)
      if (code) error.code = code[1]
      throw error
    }
    return response.value
  }
  class Buffer extends Uint8Array {
    static from(value, encoding = 'utf8') {
      if (typeof value === 'string') return new Buffer(native('codec.encode', value, encoding))
      if (value instanceof ArrayBuffer) return new Buffer(new Uint8Array(value))
      return new Buffer(value)
    }
    static alloc(size, fill = 0, encoding) {
      const result = new Buffer(size)
      if (typeof fill === 'string') {
        const pattern = Buffer.from(fill, encoding)
        for (let index = 0; index < result.length; index++)
          result[index] = pattern[index % pattern.length] || 0
      } else result.fill(fill)
      return result
    }
    static allocUnsafe(size) {
      return new Buffer(size)
    }
    static isBuffer(value) {
      return value instanceof Buffer
    }
    static byteLength(value, encoding = 'utf8') {
      return typeof value === 'string' ? Buffer.from(value, encoding).length : value.byteLength
    }
    static concat(values, total = values.reduce((n, value) => n + value.length, 0)) {
      const result = new Buffer(total)
      let index = 0
      for (const value of values) {
        const part = value.subarray(0, Math.max(0, total - index))
        result.set(part, index)
        index += part.length
      }
      return result
    }
    static compare(a, b) {
      for (let index = 0; index < Math.min(a.length, b.length); index++)
        if (a[index] !== b[index]) return a[index] < b[index] ? -1 : 1
      return Math.sign(a.length - b.length)
    }
    equals(other) {
      return Buffer.compare(this, other) === 0
    }
    toString(encoding = 'utf8', start = 0, end = this.length) {
      return native('codec.decode', Array.from(this.subarray(start, end)), encoding)
    }
    toJSON() {
      return { type: 'Buffer', data: Array.from(this) }
    }
    slice(start, end) {
      return this.subarray(start, end)
    }
  }
  globalThis.Buffer = Buffer
  const encoding = (value) => (typeof value === 'string' ? value : value?.encoding)
  const data = (value, options) =>
    Array.from(
      typeof value === 'string'
        ? Buffer.from(value, encoding(options) || 'utf8')
        : Buffer.from(value),
    )
  const stats = (value) => ({
    ...value,
    atime: new Date(value.atimeMs),
    mtime: new Date(value.mtimeMs),
    ctime: new Date(value.ctimeMs),
    birthtime: new Date(value.birthtimeMs),
    isFile: () => value.file,
    isDirectory: () => value.directory,
    isSymbolicLink: () => value.symlink,
    isBlockDevice: () => false,
    isCharacterDevice: () => false,
    isFIFO: () => false,
    isSocket: () => false,
  })
  const fs = {
    readFileSync(path, options) {
      const value = Buffer.from(native('fs.readFile', String(path)))
      return encoding(options) ? value.toString(encoding(options)) : value
    },
    writeFileSync(path, value, options) {
      return (
        native('fs.writeFile', String(path), data(value, options), options?.flag || 'w') ??
        undefined
      )
    },
    appendFileSync(path, value, options) {
      return (
        native('fs.appendFile', String(path), data(value, options), options?.flag || 'a') ??
        undefined
      )
    },
    mkdirSync(path, options) {
      return native('fs.mkdir', String(path), options?.recursive === true) ?? undefined
    },
    readdirSync(path, options) {
      const entries = native('fs.readdir', String(path))
      return options?.withFileTypes
        ? entries.map((value) => ({
            name: value.name,
            isFile: () => value.file,
            isDirectory: () => value.directory,
            isSymbolicLink: () => value.symlink,
          }))
        : entries.map((value) => value.name)
    },
    statSync(path) {
      return stats(native('fs.stat', String(path)))
    },
    lstatSync(path) {
      return stats(native('fs.lstat', String(path)))
    },
    realpathSync(path) {
      return native('fs.realpath', String(path))
    },
    accessSync(path, mode = 0) {
      return native('fs.access', String(path), mode) ?? undefined
    },
    existsSync(path) {
      try {
        native('fs.access', String(path), 0)
        return true
      } catch {
        return false
      }
    },
    renameSync(from, to) {
      return native('fs.rename', String(from), String(to)) ?? undefined
    },
    copyFileSync(from, to, flags = 0) {
      return native('fs.copyFile', String(from), String(to), flags) ?? undefined
    },
    unlinkSync(path) {
      return native('fs.unlink', String(path), false, false) ?? undefined
    },
    rmdirSync(path, options) {
      return native('fs.rmdir', String(path), options?.recursive === true, false) ?? undefined
    },
    rmSync(path, options) {
      return (
        native('fs.rm', String(path), options?.recursive === true, options?.force === true) ??
        undefined
      )
    },
    constants: { F_OK: 0, R_OK: 4, W_OK: 2, X_OK: 1, COPYFILE_EXCL: 1 },
  }
  fs.promises = {}
  for (const name of [
    'readFile',
    'writeFile',
    'appendFile',
    'mkdir',
    'readdir',
    'stat',
    'lstat',
    'realpath',
    'access',
    'rename',
    'copyFile',
    'unlink',
    'rmdir',
    'rm',
  ]) {
    fs.promises[name] = (...args) => Promise.resolve().then(() => fs[name + 'Sync'](...args))
    fs[name] = (...args) => {
      const callback = args.pop()
      if (typeof callback !== 'function') throw new TypeError('Callback must be a function')
      fs.promises[name](...args).then(
        (result) => callback(null, result),
        (error) => callback(error),
      )
    }
  }
  fs.promises.constants = fs.constants
  const separator = native('process.platform') === 'win32' ? '\\' : '/'
  const path = {
    join: (...args) => native('path.join', ...args),
    resolve: (...args) => native('path.resolve', ...args),
    normalize: (value) => native('path.normalize', value),
    dirname: (value) => native('path.dirname', value),
    basename: (value, suffix) => native('path.basename', value, suffix),
    extname: (value) => native('path.extname', value),
    isAbsolute: (value) => native('path.isAbsolute', value),
    sep: separator,
    delimiter: separator === '\\' ? ';' : ':',
  }
  const process = {
    env: {},
    cwd: () => native('process.cwd'),
    platform: native('process.platform'),
    arch: native('process.arch'),
    pid: native('process.pid'),
    nextTick: (callback, ...args) => Promise.resolve().then(() => callback(...args)),
  }
  globalThis.process = process
  globalThis.console = { log() {}, warn() {}, error() {}, info() {}, debug() {}, trace() {} }
  const timers = new Map()
  let sequence = 0
  globalThis.setTimeout = (callback, ms = 1, ...args) => {
    const value = Number(ms)
    const duration =
      !Number.isFinite(value) || value < 1 || value > 2147483647 ? 1 : Math.trunc(value)
    const timer = {
      id: ++sequence,
      time: Date.now() + duration,
      callback,
      args,
      repeat: 0,
      ref() {
        return this
      },
      unref() {
        return this
      },
      hasRef() {
        return true
      },
      [Symbol.toPrimitive]() {
        return this.id
      },
    }
    timers.set(timer.id, timer)
    return timer
  }
  globalThis.clearTimeout = (timer) =>
    timers.delete(typeof timer === 'object' ? timer?.id : Number(timer))
  globalThis.setInterval = (callback, ms, ...args) => {
    const timer = setTimeout(callback, ms, ...args)
    timer.repeat = Math.max(1, Number(ms) || 1)
    return timer
  }
  globalThis.clearInterval = globalThis.clearTimeout
  globalThis.setImmediate = (callback, ...args) => setTimeout(callback, 1, ...args)
  globalThis.clearImmediate = globalThis.clearTimeout
  globalThis.__pisperPumpTimers = () => {
    const now = Date.now()
    for (const timer of [...timers.values()]) {
      if (timer.time > now) continue
      if (timer.repeat) timer.time = now + timer.repeat
      else timers.delete(timer.id)
      timer.callback(...timer.args)
    }
  }
  const timerModule = {
    setTimeout,
    clearTimeout,
    setInterval,
    clearInterval,
    setImmediate,
    clearImmediate,
  }
  const modules = {
    'node:fs': fs,
    'node:fs/promises': fs.promises,
    'node:path': path,
    'node:buffer': { Buffer },
    'node:process': process,
    'node:timers': timerModule,
    'node:timers/promises': {
      setTimeout: (ms, value) => new Promise((resolve) => setTimeout(() => resolve(value), ms)),
    },
    'node:os': {
      platform: () => process.platform,
      arch: () => process.arch,
      tmpdir: () => native('os.tmpdir'),
      EOL: separator === '\\' ? '\r\n' : '\n',
    },
  }
  globalThis.__pisperNodeModules = modules
  const cache = new Map()
  globalThis.__pisperLoadCjs = (filename) => {
    if (cache.has(filename)) return cache.get(filename).exports
    const module = { exports: {} }
    cache.set(filename, module)
    const require = (name) => {
      const target = native('module.resolve', filename, name)
      if (target.startsWith('node:')) return modules[target]
      if (target.endsWith('.json')) return JSON.parse(native('module.read', target))
      if (target.endsWith('.mjs'))
        throw new Error('ERR_PISPER_NODE_COMPAT: require() of ESM is not implemented')
      return __pisperLoadCjs(target)
    }
    require.resolve = (name) => native('module.resolve', filename, name)
    try {
      const body = native('module.read', filename).replace(/^#![^\r\n]*/, '')
      const execute = new Function('exports', 'require', 'module', '__filename', '__dirname', body)
      execute.call(
        module.exports,
        module.exports,
        require,
        module,
        filename,
        path.dirname(filename),
      )
      return module.exports
    } catch (error) {
      cache.delete(filename)
      throw error
    }
  }
})()
