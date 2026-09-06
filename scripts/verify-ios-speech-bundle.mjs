import { lstat, readFile, readdir } from 'node:fs/promises'
import { dirname, join, relative, resolve } from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'

const projectRoot = join(dirname(fileURLToPath(import.meta.url)), '..')

export async function verifyIosSpeechBundle({ appRoot, root = projectRoot }) {
  const files = []
  let count = 0
  async function visit(directory) {
    const info = await lstat(directory)
    if (!info.isDirectory() || info.isSymbolicLink()) throw new Error('Invalid iOS app directory.')
    for (const name of await readdir(directory)) {
      if (++count > 50000) throw new Error('iOS app entry limit exceeded.')
      const path = join(directory, name)
      const entry = await lstat(path)
      if (entry.isSymbolicLink()) throw new Error('iOS app contains a symbolic link.')
      if (entry.isDirectory()) await visit(path)
      else if (entry.isFile()) files.push(path)
      else throw new Error('iOS app contains a special file.')
    }
  }
  await visit(appRoot)
  if (files.some((path) => /(?:\.onnx|(?:^|[\\/])voices(?:-selected-\d+)?\.bin)$/i.test(path))) {
    throw new Error('iOS app unexpectedly bundles speech model weights.')
  }
  const catalogs = files.filter((path) =>
    relative(appRoot, path)
      .replaceAll('\\', '/')
      .endsWith('/SpeechResources/speech-model-catalog.json'),
  )
  if (catalogs.length !== 1)
    throw new Error('iOS app must contain exactly one shared speech resource bundle.')
  const resources = dirname(catalogs[0])
  for (const name of [
    'speech-model-catalog.json',
    'speech-resource-notices.json',
    'speech-resources/xasr-bpe.vocab',
  ]) {
    const source = await readFile(join(root, 'shared', name))
    const target = await readFile(join(resources, name))
    if (!source.equals(target))
      throw new Error(`iOS speech resource differs from shared source: ${name}`)
  }
  return { resources: relative(appRoot, resources), weightsBundled: false, resourcesVerified: 3 }
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  if (process.argv.length !== 3)
    throw new Error('Usage: node scripts/verify-ios-speech-bundle.mjs <extracted .app directory>')
  console.log(JSON.stringify(await verifyIosSpeechBundle({ appRoot: resolve(process.argv[2]) })))
}
