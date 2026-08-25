import assert from 'node:assert/strict'
import { access, readdir, readFile, stat } from 'node:fs/promises'
import { join } from 'node:path'
import test from 'node:test'

const WEB_SHOT_DIRECTORY = join('docs', 'shots', 'web')
const MAX_WEB_SHOT_BYTES = 100 * 1024
const MAX_WEB_SHOTS_TOTAL_BYTES = 1.5 * 1024 * 1024

function uniqueMatches(source, pattern) {
  return [...new Set([...source.matchAll(pattern)].map((match) => match[1]))]
}

test('homepage and showcase use bounded WebP previews while preserving original links', async () => {
  const [homepage, showcase, readme, readmeEnglish] = await Promise.all([
    readFile('docs/index.html', 'utf8'),
    readFile('docs/show.html', 'utf8'),
    readFile('README.md', 'utf8'),
    readFile('README.en.md', 'utf8'),
  ])

  assert.doesNotMatch(homepage, /data-shot="shots\/(?!web\/)[^"]+\.png"/)
  assert.doesNotMatch(homepage, /<img[^>]+src="shots\/(?!web\/)[^"]+\.png"/)
  assert.doesNotMatch(showcase, /\['shots\/(?!web\/)[^']+\.png'/)
  assert.doesNotMatch(readme, /<img src="docs\/shots\/(?!web\/)[^"]+\.png"/)
  assert.doesNotMatch(readmeEnglish, /<img src="docs\/shots\/(?!web\/)[^"]+\.png"/)

  assert.match(homepage, /<a href="shots\/cli\.png"[^>]*>[\s\S]*?src="shots\/web\/cli\.webp"/)
  assert.match(
    readme,
    /<a href="docs\/shots\/chat-grid\.png"><img src="docs\/shots\/web\/chat-grid\.webp"/,
  )

  const references = [
    ...uniqueMatches(homepage, /(?:data-shot|src)="(shots\/web\/[^"]+\.webp)"/g).map((path) =>
      join('docs', path),
    ),
    ...uniqueMatches(showcase, /\['(shots\/web\/[^']+\.webp)'/g).map((path) => join('docs', path)),
    ...uniqueMatches(`${readme}\n${readmeEnglish}`, /src="(docs\/shots\/web\/[^"]+\.webp)"/g),
  ]

  assert.ok(new Set(references).size >= 20)
  for (const path of new Set(references)) await access(path)
})

test('homepage swaps desktop download calls for mobile choices at compact widths', async () => {
  const [homepage, styles] = await Promise.all([
    readFile('docs/index.html', 'utf8'),
    readFile('docs/site.css', 'utf8'),
  ])
  const mobileCalls = homepage.match(
    /<a[^>]+class="[^"]*mobile-download-cta[^"]*"[^>]+href="#mobile-downloads"[^>]*>/g,
  )

  assert.equal(mobileCalls?.length, 2)
  assert.equal(homepage.match(/下载移动端/g)?.length, 2)
  assert.equal(homepage.match(/class="[^"]*desktop-download-cta[^"]*"/g)?.length, 2)
  assert.match(
    homepage,
    /class="nav-mobile-github magnetic"[\s\S]*href="https:\/\/github\.com\/ling-kong-ran\/pisper"/,
  )
  assert.match(homepage, /id="mobile-downloads"/)
  assert.match(styles, /\.mobile-download-cta\s*\{\s*display: none;/)
  assert.match(
    styles,
    /@media \(max-width: 960px\)[\s\S]*?\.nav-links,\s*\.desktop-download-cta\s*\{\s*display: none;/,
  )
  assert.match(
    styles,
    /@media \(max-width: 960px\)[\s\S]*?\.nav-mobile-github,[\s\S]*?\.mobile-download-cta\s*\{\s*display: inline-flex;/,
  )
  assert.match(styles, /@media \(max-width: 620px\)[\s\S]*?\.nav \{\s*gap: 8px;/)
})

test('homepage WebP previews stay within the loading budget', async () => {
  const files = (await readdir(WEB_SHOT_DIRECTORY)).filter((name) => name.endsWith('.webp'))
  let totalBytes = 0

  assert.ok(files.length >= 20)
  for (const name of files) {
    const path = join(WEB_SHOT_DIRECTORY, name)
    const [metadata, contents] = await Promise.all([stat(path), readFile(path)])
    totalBytes += metadata.size
    assert.ok(metadata.size <= MAX_WEB_SHOT_BYTES, `${path} exceeds the per-image budget`)
    assert.equal(contents.subarray(0, 4).toString('ascii'), 'RIFF')
    assert.equal(contents.subarray(8, 12).toString('ascii'), 'WEBP')
  }

  assert.ok(totalBytes <= MAX_WEB_SHOTS_TOTAL_BYTES, 'homepage screenshots exceed total budget')
})
