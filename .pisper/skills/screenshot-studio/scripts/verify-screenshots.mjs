// Verify every captured PNG before replacing any file under docs/shots/.
import { copyFileSync, readFileSync, statSync } from 'node:fs'
import { resolve } from 'node:path'
import { RUN_DIR, SHOTS_DIR } from './screenshot-config.mjs'
import { WEB_SHOTS } from './web-shots.mjs'
const EXPECTED_SIZE = '2558x1380'
const PNG_SIGNATURE = Buffer.from([137, 80, 78, 71, 13, 10, 26, 10])

function dimensions(file) {
  const data = readFileSync(file)
  if (data.length < 24 || !data.subarray(0, 8).equals(PNG_SIGNATURE)) {
    throw new Error('invalid PNG signature')
  }
  if (data.toString('ascii', 12, 16) !== 'IHDR') throw new Error('missing PNG IHDR')
  return `${data.readUInt32BE(16)}x${data.readUInt32BE(20)}`
}

const verified = []
let failed = false
for (const name of WEB_SHOTS) {
  const source = resolve(RUN_DIR, `${name}.png`)
  const thumbnail = resolve(RUN_DIR, 'web', `${name}.webp`)
  try {
    statSync(source)
    const size = dimensions(source)
    if (size !== EXPECTED_SIZE) {
      console.error(`BAD SIZE ${name}.png -> ${size} (expected ${EXPECTED_SIZE})`)
      failed = true
      continue
    }
    statSync(thumbnail)
    const webpHeader = readFileSync(thumbnail).subarray(0, 12).toString('ascii')
    if (webpHeader.slice(0, 4) !== 'RIFF' || webpHeader.slice(8, 12) !== 'WEBP') {
      throw new Error('invalid WebP RIFF header')
    }
    verified.push({ name, source, thumbnail, size })
  } catch (error) {
    console.error(`INVALID ${name}.png -> ${error instanceof Error ? error.message : String(error)}`)
    failed = true
  }
}

if (failed || verified.length !== WEB_SHOTS.length) {
  console.error('Screenshot verification failed; docs/shots was not modified.')
  process.exit(1)
}

for (const { name, source, thumbnail, size } of verified) {
  copyFileSync(source, resolve(SHOTS_DIR, `${name}.png`))
  copyFileSync(thumbnail, resolve(SHOTS_DIR, 'web', `${name}.webp`))
  console.log(`replaced docs/shots/${name}.png and web/${name}.webp (${size})`)
}
console.log(`All ${verified.length} Web screenshots and thumbnails verified and replaced.`)
