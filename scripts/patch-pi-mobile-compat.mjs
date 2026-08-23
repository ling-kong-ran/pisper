// Pi 的 headless 核心会静态导入移动 Node 未提供的内建模块，导致会话代码尚未
// 运行就加载失败。补丁只把可选宿主能力改为惰性探测，完整 Node 上保持原行为。
import { readFile, writeFile } from 'node:fs/promises'
import { join } from 'node:path'
import process from 'node:process'

const root = join(process.cwd(), 'node_modules', '@earendil-works', 'pi-coding-agent', 'dist')

const patches = [
  {
    file: 'utils/child-process.js',
    replacements: [
      {
        before:
          'import { spawn as nodeSpawn, spawnSync as nodeSpawnSync, } from "node:child_process";\nimport crossSpawn from "cross-spawn";',
        after: `let nodeSpawn;
let nodeSpawnSync;
let crossSpawn;
try {
    const childProcess = await import("node:child_process");
    nodeSpawn = childProcess.spawn;
    nodeSpawnSync = childProcess.spawnSync;
    if (process.platform === "win32")
        crossSpawn = (await import("cross-spawn")).default;
}
catch { }`,
      },
      {
        before: `export function spawnProcess(command, args, options) {
    return process.platform === "win32" ? crossSpawn(command, args, options) : nodeSpawn(command, args, options);
}
export function spawnProcessSync(command, args, options) {
    return process.platform === "win32"
        ? crossSpawn.sync(command, args, options)
        : nodeSpawnSync(command, args, options);
}`,
        after: `function unavailableError() {
    const error = new Error("Child processes are unavailable in this runtime");
    error.code = "ERR_PISPER_CAPABILITY_UNAVAILABLE";
    return error;
}
export function spawnProcess(command, args, options) {
    if (process.platform === "win32" && crossSpawn)
        return crossSpawn(command, args, options);
    if (nodeSpawn)
        return nodeSpawn(command, args, options);
    throw unavailableError();
}
export function spawnProcessSync(command, args, options) {
    if (process.platform === "win32" && crossSpawn)
        return crossSpawn.sync(command, args, options);
    if (nodeSpawnSync)
        return nodeSpawnSync(command, args, options);
    return { status: null, signal: null, stdout: "", stderr: "", error: unavailableError() };
}`,
      },
    ],
  },
  {
    file: 'core/resolve-config-value.js',
    replacements: [
      {
        before: 'import { execSync, spawnSync } from "child_process";',
        after: `let execSync;
let spawnSync;
try {
    const childProcess = await import("node:child_process");
    execSync = childProcess.execSync;
    spawnSync = childProcess.spawnSync;
}
catch { }`,
      },
      {
        before: `function executeWithConfiguredShell(command) {
    try {
        const { shell, args, commandTransport } = getShellConfig();`,
        after: `function executeWithConfiguredShell(command) {
    try {
        if (!spawnSync)
            return { executed: false, value: undefined };
        const { shell, args, commandTransport } = getShellConfig();`,
      },
      {
        before: `function executeWithDefaultShell(command) {
    try {
        const output = execSync(command, {`,
        after: `function executeWithDefaultShell(command) {
    try {
        if (!execSync)
            return undefined;
        const output = execSync(command, {`,
      },
    ],
  },
  {
    file: 'core/exec.js',
    replacements: [
      {
        before:
          'import { spawn } from "node:child_process";\nimport { waitForChildProcess } from "../utils/child-process.js";',
        after: 'import { spawnProcess, waitForChildProcess } from "../utils/child-process.js";',
      },
      {
        before: '        const proc = spawn(command, args, {',
        after: '        const proc = spawnProcess(command, args, {',
      },
    ],
  },
  ...['core/tools/grep.js', 'core/tools/find.js'].map((file) => ({
    file,
    replacements: [
      {
        before: 'import { spawn } from "child_process";',
        after: 'import { spawnProcess as spawn } from "../../utils/child-process.js";',
      },
    ],
  })),
  {
    file: 'utils/image-resize.js',
    replacements: [
      {
        before: 'import { Worker } from "node:worker_threads";',
        after: `let Worker;
try {
    Worker = (await import("node:worker_threads")).Worker;
}
catch { }`,
      },
      {
        before: `function createResizeWorker(workerSpecifier) {
    return new Worker(workerSpecifier);
}`,
        after: `function createResizeWorker(workerSpecifier) {
    if (!Worker)
        throw new Error("Worker threads are unavailable in this runtime");
    return new Worker(workerSpecifier);
}`,
      },
    ],
  },
]

let changed = 0
for (const patch of patches) {
  const path = join(root, patch.file)
  let source = await readFile(path, 'utf8')
  let fileChanged = false
  for (const replacement of patch.replacements) {
    if (source.includes(replacement.after)) continue
    if (!source.includes(replacement.before)) {
      throw new Error(`Pi mobile compatibility patch no longer matches ${patch.file}`)
    }
    source = source.replace(replacement.before, replacement.after)
    fileChanged = true
  }
  if (fileChanged) {
    await writeFile(path, source, 'utf8')
    changed += 1
  }
}

console.log(`Pi mobile compatibility patch ready (${changed} files changed).`)
