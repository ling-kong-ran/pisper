import { mkdir, readFile } from 'node:fs/promises'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import sharp from 'sharp'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const source = await readFile(join(root, 'design', 'pisper', 'pisper-logo-icon-source.png'))
const buildDir = join(root, 'build')
const iconsDir = join(buildDir, 'icons')
const sizes = [16, 24, 32, 48, 64, 128, 256, 512, 1024]

await mkdir(iconsDir, { recursive: true })
await sharp(source, { density: 768 }).resize(1024, 1024).png().toFile(join(buildDir, 'icon.png'))
await Promise.all(sizes.map((size) => sharp(source, { density: 768 })
  .resize(size, size)
  .png()
  .toFile(join(iconsDir, `${size}x${size}.png`))))

console.log(`Generated Pisper desktop icons in ${buildDir}`)
