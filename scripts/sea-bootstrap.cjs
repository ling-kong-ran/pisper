const { existsSync } = require('node:fs')
const { dirname, join, resolve } = require('node:path')
const { pathToFileURL } = require('node:url')

const executableDirectory = dirname(process.execPath)
const appRoot = resolve(process.env.PISPER_APP_ROOT || join(executableDirectory, 'runtime'))
const entrypoint = join(appRoot, 'runtime', 'sidecar.mjs')

if (!existsSync(entrypoint)) {
  console.error(`Pisper sidecar runtime was not found at ${entrypoint}`)
  process.exit(1)
}

process.env.PISPER_APP_ROOT = appRoot
import(pathToFileURL(entrypoint).href).catch((error) => {
  console.error(error)
  process.exit(1)
})
