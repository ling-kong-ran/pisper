import assert from 'node:assert/strict'
import { setImmediate } from 'node:timers/promises'
import test from 'node:test'
import { speechSegments } from '../../src/features/chat/speech-text.ts'
import { streamingSpeechSegments } from '../../src/features/chat/speech-stream-text.ts'

const compact = (text) => text.replace(/\s+/g, '')
const cost = (text) => Array.from(text).length
function assertCleanContent(actual, expected, message) {
  const body = (text) => text.replace(/[\p{P}\p{Z}\s]/gu, '')
  assert.equal(body(actual), body(expected), message)
}
const signal = () => new AbortController().signal
async function* chunks(values) {
  yield* values
}
async function collect(values, limit = 16) {
  const result = []
  for await (const segment of streamingSpeechSegments(chunks(values), signal(), limit))
    result.push(segment)
  return result
}
function deferred() {
  let resolve, reject
  const promise = new Promise((yes, no) => {
    resolve = yes
    reject = no
  })
  return { promise, resolve, reject }
}
function source() {
  const waiting = []
  let reads = 0
  let returned = 0
  return {
    [Symbol.asyncIterator]() {
      return this
    },
    next() {
      reads++
      const pending = deferred()
      waiting.push(pending)
      return pending.promise
    },
    return() {
      returned++
      return new Promise(() => {})
    },
    get reads() {
      return reads
    },
    get returned() {
      return returned
    },
    push(value) {
      assert.ok(waiting.length, 'the consumer must be waiting for the source')
      waiting.shift().resolve({ value, done: false })
    },
    finish() {
      assert.ok(waiting.length)
      waiting.shift().resolve({ done: true })
    },
    fail(error) {
      assert.ok(waiting.length)
      waiting.shift().reject(error)
    },
  }
}
async function tick() {
  for (let index = 0; index < 3; index++) await setImmediate()
}
function observe(promise) {
  const state = { pending: true }
  state.done = promise.then(
    (value) => Object.assign(state, { pending: false, value }),
    (error) => Object.assign(state, { pending: false, error }),
  )
  return state
}

const fixtures = [
  '第一句说完了。第二句话！最后没有标点',
  'A full sentence. Another sentence! The final words',
  'Dr. Smith paid 3.14 dollars. Mr. Jones uses e.g. examples in the U.S. office. Tail',
  '# Heading\n\n- First **bold words** and *emphasis*.\n- Second ~~removed style~~ words!\n\nTail',
  'Before **nested [a *label*](https://example.test/secret?q=1.5)** after. Tail',
  'First.\n\n```js\nSECRET_BLOCK!\n```\n\nSecond. Tail',
  'First.\n\n~~~js\nSECRET_TILDE?\n~~~\n\nSecond! Tail',
  'Use `npm install` now. Do not read `x = SECRET();` please! Tail',
  'Use ``a ` tick`` now. `' + 'LONG_CODE'.repeat(10) + '` Tail.',
  'Before <script>SECRET_SCRIPT.\n\nStill secret!</script> after. Tail',
  'Before <style>SECRET_STYLE.\n\nStill secret!</style> after! Tail',
  'Before <span>visible &amp; stable</span> after. Tail',
  '<div>SECRET_HTML. Never speak!</div>\n\nVisible. Tail',
  'First [reference][target]. Tail\n\n[target]: https://example.test/secret',
  '![SECRET_IMAGE.](https://example.test/image.png) Visible. Tail',
  '[![SECRET_IMAGE](https://example.test/image)](https://example.test/url) Visible. Tail',
  'A &amp; B &#46; Another &#x3002; Tail',
  'Escaped \\*stars\\* and \\[brackets\\] are literal. Tail',
  '| Name | Value |\n| --- | --- |\n| alpha. | beta! |\n\nTail',
  '> Quoted **text**.\n> Next sentence!\n\nTail',
  'Plain https://example.test/SECRET?q=1.5 link. Tail',
  '这是没有标点并且很长的中文前缀 https://example.test/SECRET 后续内容继续完整保留',
  'Alpha beta gamma delta epsilon 1,000 dollars e.g. in the U.S. office. Tail',
  '𠀀文𠀁字。Music 𝄞 stays! Emoji 😀 is removed. Tail',
  'Really?! Yes!!! “Quoted sentence.” Next... Tail',
  '```js\nSECRET_UNCLOSED. Never speak!',
  'Before <script>SECRET_UNCLOSED. Never speak!',
  'Before <template><b>SECRET_TEMPLATE!</b></template> after. Tail',
  'Before <svg><text>SECRET_SVG!</text></svg> after. Tail',
  'Before <!-- SECRET_COMMENT! --> after. Tail',
  'Before **1. nested text! [label](https://example.test/hidden)** after. Tail',
  'First. Before [label with punctuation!](https://example.test/hidden) Next',
  'Literal \\<script> is escaped text. Tail',
  'An unmatched [label. still literal at EOF',
  'An unmatched `inline. still literal at EOF',
  'Before <script>SECRET [ ` * !</script> 后续第一句。第二句！',
  'Before <style>SECRET [ ` * !</style> 后续第一句。第二句！',
  'Before <iframe>SECRET!</iframe> 后续第一句。第二句！',
  'Before <object><b>SECRET!</b></object> 后续第一句。第二句！',
  'Before <math><mtext>SECRET!</mtext></math> 后续第一句。第二句！',
  'Before <noscript>SECRET!</noscript> 后续第一句。第二句！',
  'Before <pre>SECRET!</pre> 后续第一句。第二句！',
  'Before <template><span>SECRET [ * !</span></template> 后续第一句。第二句！',
  'Before <br> 后续第一句。第二句！',
  'Before <svg/> 后续第一句。第二句！',
  'Before <span title="SECRET > !">visible</span> 后续第一句。第二句！',
  '<!-- SECRET! -->\n\n后续第一句。第二句！',
  'Before ![SECRET_IMAGE!](https://example.test/image) 后续第一句。第二句！',
  'Before <img src="SECRET"> [label](https://example.test/SECRET!) 后续第一句。',
  'Before <svg/> `SECRET = code();` 后续第一句。',
]

for (const markdown of fixtures) {
  test(`every UTF-16 split preserves full cleaned content: ${markdown.slice(0, 48)}`, async () => {
    const expected = compact(speechSegments(markdown, 32).join(''))
    for (let split = 0; split <= markdown.length; split++) {
      const actual = await collect([markdown.slice(0, split), markdown.slice(split)], 32)
      assertCleanContent(actual.join(''), expected, `split ${split}`)
      assert.ok(actual.every((segment) => cost(segment) <= 32 && segment.isWellFormed()))
      assert.ok(actual.every((segment) => !/^[\p{P}\s]+$/u.test(segment)))
      assert.doesNotMatch(actual.join(''), /SECRET|LONG_CODE/)
    }
    assertCleanContent((await collect(markdown.split(''), 32)).join(''), expected)
  })
}

test('first sentence yields while real source remains suspended, without reading ahead', async () => {
  const input = source()
  const controller = new AbortController()
  const output = streamingSpeechSegments(input, controller.signal)
  const first = observe(output.next())
  await tick()
  input.push('第一句完整。后')
  await first.done
  assert.deepEqual(first.value, { value: '第一句完整。', done: false })
  assert.equal(input.reads, 1)
  await tick()
  assert.equal(input.reads, 1, 'a suspended yield must not prefetch another delta')
  const tail = observe(output.next())
  await tick()
  assert.equal(tail.pending, true)
  input.push('面的无标点尾句')
  await tick()
  assert.equal(tail.pending, true)
  input.finish()
  await tail.done
  assert.deepEqual(tail.value, { value: '后面的无标点尾句', done: false })
  assert.deepEqual(await output.next(), { value: undefined, done: true })
})

for (const sentence of ['第一句。', '第一句！', '第一句？', 'First!', 'First?'])
  test(`sentence at the exact chunk end yields without another source read: ${sentence}`, async () => {
    const input = source()
    const output = streamingSpeechSegments(input, signal())
    const first = observe(output.next())
    await tick()
    input.push(sentence)
    await tick()
    assert.equal(first.pending, false, 'first next must resolve while no more data or EOF exists')
    assert.deepEqual(first.value, { value: sentence, done: false })
    assert.equal(input.reads, 1)
    await output.return()
    assert.equal(input.returned, 1)
  })

test('late repeated sentence marks and quotes never synthesize without text', async () => {
  for (const tail of ['', '下一句！']) {
    const input = source()
    const output = streamingSpeechSegments(input, signal(), 32)
    const first = observe(output.next())
    await tick()
    input.push('第一句。')
    await tick()
    assert.deepEqual(first.value, { value: '第一句。', done: false })
    const next = observe(output.next())
    await tick()
    for (const punctuation of ['！', '？', '”']) {
      input.push(punctuation)
      await tick()
      assert.equal(next.pending, true)
    }
    if (tail) {
      input.push(tail)
      await tick()
      assert.deepEqual(next.value, { value: `！？”${tail}`, done: false })
      await output.return()
    } else {
      input.finish()
      await tick()
      assert.deepEqual(next.value, { value: undefined, done: true })
    }
  }
})

test('long sentence is bounded, preserves words, and expands only its own segments', async () => {
  const input = source()
  const controller = new AbortController()
  const output = streamingSpeechSegments(input, controller.signal, 16)
  const first = output.next()
  await tick()
  input.push('Alpha beta gamma delta epsilon. Next sentence. Tail')
  const seen = [(await first).value]
  while (!seen.at(-1).endsWith('.')) seen.push((await output.next()).value)
  assert.deepEqual(
    seen.flatMap((part) => part.match(/[A-Za-z]+/g)),
    ['Alpha', 'beta', 'gamma', 'delta', 'epsilon'],
  )
  assert.ok(seen.every((part) => cost(part) <= 16))
  assert.equal(input.reads, 1)
  assert.equal((await output.next()).value, 'Next sentence.')
  await output.return()
  assert.equal(input.returned, 1)
})

test('decimal and abbreviations do not count as sentence or fragment boundaries', async () => {
  const input = source()
  const controller = new AbortController()
  const output = streamingSpeechSegments(input, controller.signal, 160)
  const first = observe(output.next())
  await tick()
  for (const value of ['Dr. ', 'Smith has 3.', '14 dollars e.g. ', 'in the U.S. ', 'office']) {
    input.push(value)
    await tick()
    assert.equal(first.pending, true, value)
  }
  input.push('. Next')
  await first.done
  assert.equal(first.value.value, 'Dr. Smith has 3.14 dollars e.g. in the U.S. office.')
  await output.return()
})

for (const [head, middle, tail] of [
  ['`', 'SECRET = code();. ', '` Visible. Next'],
  ['``', '`js\nSECRET.\n', '```\n\nVisible. Next'],
  ['Before <scr', 'ipt>SECRET.\n\n', '</script> Visible. Next'],
  ['Before <sty', 'le>SECRET.\n\n', '</style> Visible. Next'],
  ['Before [label](', 'https://example.test/SECRET. ', ') Visible. Next'],
  ['Before ![', 'SECRET. ', '](https://example.test/x) Visible. Next'],
  ['Before <img> [label](', 'https://example.test/SECRET! ', ') Visible. Next'],
  ['Before <svg/> `', 'SECRET = code();! ', '` Visible. Next'],
])
  test(`unfinished markup cannot leak before source completion: ${head}`, async () => {
    const input = source()
    const controller = new AbortController()
    const output = streamingSpeechSegments(input, controller.signal, 160)
    const first = observe(output.next())
    await tick()
    input.push(head)
    await tick()
    input.push(middle)
    await tick()
    assert.equal(first.pending, true)
    input.push(tail)
    await tick()
    if (first.pending) input.finish()
    await first.done
    assert.equal(first.error, undefined)
    assert.doesNotMatch(first.value.value, /SECRET/)
    await output.return()
  })

for (const [head, close, expected] of [
  ['<div>SECRET!', '</div>\n\n', ''],
  ['<script>SECRET!\n\n', '</script>\n\n', ''],
  ['前文 <script>SECRET [ ` * !', '</script> ', '前文 '],
  ['前文 <style>SECRET [ ` * !', '</style> ', '前文 '],
  ['前文 <template><b>SECRET [ * !</b>', '</template> ', '前文 '],
  ['前文 <svg><text>SECRET!</text>', '</svg> ', '前文 '],
  ['前文 <math><mtext>SECRET!</mtext>', '</math> ', '前文 '],
  ['前文 <iframe>SECRET!', '</iframe> ', '前文 '],
  ['前文 <object>SECRET!', '</object> ', '前文 '],
  ['前文 <noscript>SECRET!', '</noscript> ', '前文 '],
  ['前文 <pre>SECRET!', '</pre> ', '前文 '],
  ['前文 <!-- SECRET!', '--> ', '前文 '],
  ['前文 <span>可见', '</span> ', '前文 可见 '],
])
  test(`closed HTML resumes consecutive sentences before EOF: ${head}`, async () => {
    const input = source()
    const output = streamingSpeechSegments(input, signal(), 160)
    const first = observe(output.next())
    await tick()
    input.push(head)
    await tick()
    assert.equal(first.pending, true)
    input.push(`${close}后续第一句。`)
    await tick()
    assert.equal(first.pending, false, 'a closed HTML region must not postpone speech until EOF')
    assert.deepEqual(first.value, { value: `${expected}后续第一句。`, done: false })
    assert.equal(input.reads, 2)
    const second = observe(output.next())
    await tick()
    input.push('第二句！')
    await tick()
    assert.deepEqual(second.value, { value: '第二句！', done: false })
    assert.equal(input.reads, 3)
    await output.return()
  })

test('all punctuation split boundaries coalesce without punctuation-only synthesis', async () => {
  for (const markdown of [
    'Really?! Next!!! Tail',
    '你好！！！下一句？？？尾句',
    '“Hello.” Next. Tail',
  ]) {
    const actual = await collect(markdown.split(''), 16)
    assert.equal(compact(actual.join('')), compact(markdown))
    assert.ok(actual.every((part) => !/^[\p{P}\s]+$/u.test(part)))
  }
  assert.deepEqual(await collect(['!!!']), [])
  assert.deepEqual(await collect(['第一句。', '！”'.repeat(40)]), ['第一句。'])
  await assert.rejects(collect(['a' + '!'.repeat(40)]), /punctuation.*limit/i)
})

test('encoded sentence marks stream before EOF without speaking partial entities', async () => {
  const input = source()
  const controller = new AbortController()
  const output = streamingSpeechSegments(input, controller.signal, 160)
  const first = observe(output.next())
  await tick()
  input.push('Words ')
  await tick()
  assert.deepEqual(first.value, { value: 'Words', done: false })
  const next = observe(output.next())
  await tick()
  for (const value of ['\ncontinued ', '&', '#', 'x', '3', '0', '0', '2']) {
    input.push(value)
    await tick()
    assert.equal(next.pending, true)
  }
  input.push(';')
  await tick()
  assert.equal(next.pending, false, 'decoded Chinese sentence mark must yield immediately')
  assert.equal(next.value.value, 'continued 。')
  await output.return()
})

test('quotes on oversized tokens preserve text with bounded non-punctuation segments', async () => {
  for (const markdown of [
    '“extraordinaryword.” Next',
    'A “extraordinaryword.” Next',
    'abcd!!! Next',
  ]) {
    const actual = await collect(markdown.split(''), 8)
    assert.equal(compact(actual.join('')), compact(markdown))
    assert.ok(actual.every((part) => cost(part) <= 8 && !/^[\p{P}\s]+$/u.test(part)))
  }
})

for (const deltas of [['你好，'], ['你', '好', '，'], ['Hello '], ['Hel', 'lo', ' ']])
  test(`closed first phrase yields immediately before another delta: ${deltas.join('|')}`, async (t) => {
    t.mock.timers.enable({ apis: ['setTimeout', 'Date'] })
    const input = source()
    const output = streamingSpeechSegments(input, signal())
    const first = observe(output.next())
    await tick()
    for (const [index, delta] of deltas.entries()) {
      input.push(delta)
      await tick()
      assert.equal(first.pending, index < deltas.length - 1)
    }
    const phrase = deltas.join('')
    assert.deepEqual(first.value, { value: phrase.trim(), done: false })
    assert.equal(input.reads, deltas.length, 'first phrase must not await another delta or timer')
    const next = observe(output.next())
    await tick()
    for (const delta of ['今天', '很高兴']) {
      input.push(delta)
      await tick()
      assert.equal(next.pending, true, 'only the first phrase bypasses the normal buffer')
    }
    input.push('。')
    await tick()
    assert.deepEqual(next.value, { value: '今天很高兴。', done: false })
    assert.equal(first.value.value + next.value.value, `${phrase.trim()}今天很高兴。`)
    const done = output.next()
    await tick()
    input.finish()
    assert.deepEqual(await done, { value: undefined, done: true })
  })

for (const text of [
  '你好',
  'Hello',
  'https:',
  'https: ',
  '1,000 ',
  '3.14 ',
  'Hello https://example.test/ ',
  'Hello [label](https://example.test/ ',
  'Hello <script>SECRET ',
  'Hello <script>SECRET</script> ',
  'Hello `SECRET = code();` ',
  'Hello **unfinished ',
  '```js\nSECRET \n',
])
  test(`source whitespace cannot bypass incomplete or excluded text: ${text}`, async (t) => {
    t.mock.timers.enable({ apis: ['setTimeout', 'Date'] })
    const input = source()
    const controller = new AbortController()
    const output = streamingSpeechSegments(input, controller.signal, 160)
    const first = observe(output.next())
    await tick()
    input.push(text)
    await tick()
    assert.equal(first.pending, true)
    controller.abort()
    await first.done
    assert.equal(first.error.name, 'AbortError')
  })

for (const whitespace of [' ', '\t', '\n', '\u00a0'])
  test(`only the first whitespace-closed phrase bypasses the fragment deadline: ${JSON.stringify(whitespace)}`, async (t) => {
    t.mock.timers.enable({ apis: ['setTimeout', 'Date'] })
    const input = source()
    const output = streamingSpeechSegments(input, signal())
    const first = observe(output.next())
    await tick()
    input.push(`Hello${whitespace}`)
    await tick()
    assert.deepEqual(first.value, { value: 'Hello', done: false })
    const next = observe(output.next())
    await tick()
    input.push('world ')
    await tick()
    assert.equal(next.pending, true)
    t.mock.timers.tick(249)
    await tick()
    assert.equal(next.pending, true)
    t.mock.timers.tick(1)
    await tick()
    assert.deepEqual(next.value, { value: 'world', done: false })
    const done = output.next()
    await tick()
    input.finish()
    assert.deepEqual(await done, { value: undefined, done: true })
  })

for (const phrase of [
  '好的，',
  '嗯，',
  'Sure,',
  '我已经看到你说的问题，',
  '我们先检查语音输出；',
  'Let me check, ',
])
  test(`quick-yield releases a clause before the sentence ends: ${phrase}`, async () => {
    const input = source()
    const output = streamingSpeechSegments(input, signal())
    const first = observe(output.next())
    await tick()
    input.push(phrase)
    await tick()
    assert.equal(first.pending, false)
    assert.equal(compact(first.value.value), compact(phrase))
    assert.equal(input.reads, 1)
    await output.return()
  })

for (const text of [
  '这是一个没有任何句号但是仍然应该尽早开始播报的长句',
  'Alpha beta gamma delta epsilon zeta without punctuation',
])
  test(`long unpunctuated text yields bounded words before EOF: ${text}`, async () => {
    const input = source()
    const output = streamingSpeechSegments(input, signal())
    const first = observe(output.next())
    await tick()
    input.push(text)
    await tick()
    assert.equal(first.pending, false)
    assert.ok(cost(first.value.value) <= 16)
    assert.ok(text.startsWith(first.value.value))
    assert.equal(input.reads, 1)
    await output.return()
    assertCleanContent((await collect(text.split(''))).join(''), text)
  })

test('the fragment deadline releases stable words without losing the outstanding source read', async (t) => {
  t.mock.timers.enable({ apis: ['setTimeout', 'Date'] })
  const input = source()
  const output = streamingSpeechSegments(input, signal())
  const first = observe(output.next())
  await tick()
  input.push('Alpha be')
  await tick()
  t.mock.timers.tick(200)
  input.push('ta gam')
  await tick()
  t.mock.timers.tick(49)
  await tick()
  assert.equal(first.pending, true)
  t.mock.timers.tick(1)
  await tick()
  assert.deepEqual(first.value, { value: 'Alpha beta', done: false })
  assert.equal(input.reads, 3)
  const next = observe(output.next())
  await tick()
  assert.equal(input.reads, 3, 'a timer flush must reuse, not replace, the pending next')
  input.push('ma。')
  await tick()
  assert.deepEqual(next.value, { value: 'gamma。', done: false })
  await output.return()
  assert.equal(input.returned, 1)
})

for (const late of ['resolve', 'reject'])
  test(`abort after a timed fragment observes the late source ${late}`, async (t) => {
    t.mock.timers.enable({ apis: ['setTimeout', 'Date'] })
    const input = source()
    const controller = new AbortController()
    const output = streamingSpeechSegments(input, controller.signal)
    const first = observe(output.next())
    await tick()
    input.push('Alpha beta')
    await tick()
    t.mock.timers.tick(250)
    await tick()
    assert.equal(first.value.value, 'Alpha')
    const next = observe(output.next())
    await tick()
    controller.abort()
    await next.done
    assert.equal(next.error.name, 'AbortError')
    if (late === 'resolve') input.push(' late。')
    else input.fail(new Error('late source failure'))
    await tick()
    assert.equal(input.returned, 1)
  })

for (const head of [
  'Alpha https:',
  'Alpha [SECRET',
  'Alpha `SECRET',
  'Alpha <script>SECRET',
  'Alpha &amp',
])
  test(`timed fragments retain unfinished markup and URL prefixes: ${head}`, async (t) => {
    t.mock.timers.enable({ apis: ['setTimeout', 'Date'] })
    const input = source()
    const output = streamingSpeechSegments(input, signal())
    const first = observe(output.next())
    await tick()
    input.push(head)
    await tick()
    t.mock.timers.tick(250)
    await tick()
    if (!first.pending) assert.equal(first.value.value, 'Alpha')
    const next = first.pending ? first : observe(output.next())
    await tick()
    input.finish()
    await next.done
    await output.return()
  })

test('timed UTF-16 splits preserve cleaned content across markup, links and word continuations', async (t) => {
  t.mock.timers.enable({ apis: ['setTimeout', 'Date'] })
  for (const markdown of [
    'Alpha beta gamma https://example.test/SECRET Next sentence。',
    'Alpha beta [label](https://example.test/SECRET) 后续内容保留。',
    'Alpha beta <script>SECRET [ * !</script> 后续内容保留。',
    '这是很长的没有标点的中文正文随后出现 `SECRET = code();` 最后还有内容。',
    'Alpha beta 1,000 dollars and 3.14 euros。',
    '𠀀文𠀁字和普通中文字符都不能因为提前播报而丢失。',
  ]) {
    for (let split = 0; split <= markdown.length; split++) {
      const input = source()
      const actual = []
      const consuming = (async () => {
        for await (const segment of streamingSpeechSegments(input, signal())) actual.push(segment)
      })()
      await tick()
      for (const part of [markdown.slice(0, split), markdown.slice(split)]) {
        input.push(part)
        await tick()
        t.mock.timers.tick(250)
        await tick()
      }
      input.finish()
      await consuming
      assertCleanContent(actual.join(''), speechSegments(markdown).join(''), `split ${split}`)
      assert.doesNotMatch(actual.join(''), /SECRET|https:/)
      assert.ok(actual.every((segment) => cost(segment) <= 16 && segment.isWellFormed()))
    }
  }
})

test('numeric commas are not mistaken for quick-yield clause delimiters', async () => {
  const input = source()
  const output = streamingSpeechSegments(input, signal(), 160)
  const first = observe(output.next())
  await tick()
  input.push('The cost is 1,')
  await tick()
  assert.equal(first.pending, true)
  input.push('000 dollars')
  await tick()
  assert.equal(first.pending, true)
  input.push('。')
  await tick()
  assert.equal(first.value.value, 'The cost is 1,000 dollars。')
  await output.return()
})

test('empty input, excluded input, and EOF tail have deterministic completion', async () => {
  assert.deepEqual(await collect([]), [])
  assert.deepEqual(await collect(['', '', '']), [])
  assert.deepEqual(await collect(['```js\nSECRET.\n```']), [])
  assert.deepEqual(await collect(['无标点', '尾句']), ['无标点尾句'])
  assert.deepEqual(await collect(['a'.repeat(100)], 8), [...Array(12).fill('aaaaaaaa'), 'aaaa'])
})

test('limits reject invalid numeric configuration and cumulative UTF-16 or segment overflow', async () => {
  for (const limit of [0, 7, 161, 8.5, NaN, Infinity, -Infinity])
    await assert.rejects(collect(['text'], limit), /limit/i)
  for (const limit of [8, 16, 160]) {
    const actual = await collect(['a'.repeat(1000)], limit)
    assert.equal(actual.join(''), 'a'.repeat(1000))
    assert.ok(actual.every((value) => cost(value) <= limit))
  }
  await assert.rejects(collect(['a'.repeat(16000), 'b'.repeat(16001)]), /limit/i)
  assert.equal((await collect(['𠀀'.repeat(16000)], 160)).join(''), '𠀀'.repeat(16000))
  await assert.rejects(collect(['𠀀'.repeat(16000), 'x'], 160), /limit/i)
  assert.equal((await collect(['Hello. '.repeat(512)])).length, 512)
  await assert.rejects(collect(['Hello. '.repeat(513)]), /too many|limit/i)
})

test('split surrogate pairs remain valid, lone surrogates and non-string deltas reject', async () => {
  assert.equal((await collect(['\ud840', '', '\udc00。后', '文'])).join(''), '𠀀。后文')
  for (const values of [
    ['\ud800'],
    ['\udc00'],
    ['\ud800', 'x'],
    ['\ud800', '\ud800'],
    [42],
    [null],
  ])
    await assert.rejects(collect(values), /Unicode|delta/i)
})

for (const late of ['resolve', 'reject'])
  test(`abort wakes hung next, invokes return, and observes late ${late}`, async () => {
    const input = source()
    const controller = new AbortController()
    const output = streamingSpeechSegments(input, controller.signal)
    const first = observe(output.next())
    await tick()
    assert.equal(input.reads, 1)
    controller.abort()
    await first.done
    assert.equal(first.error.name, 'AbortError')
    assert.equal(input.returned, 1)
    if (late === 'resolve') input.push('Late text.')
    else input.fail(new Error('Late failure'))
    await tick()
    assert.deepEqual(await output.next(), { value: undefined, done: true })
  })

test('abort between yielded segments stops the current sentence and closes the input', async () => {
  const input = source()
  const controller = new AbortController()
  const output = streamingSpeechSegments(input, controller.signal, 8)
  const first = output.next()
  await tick()
  input.push('Alpha beta gamma delta. Next')
  assert.equal((await first).value, 'Alpha')
  controller.abort()
  await assert.rejects(output.next(), { name: 'AbortError' })
  assert.equal(input.returned, 1)
  assert.equal(input.reads, 1)
})

test('pre-abort does not acquire an iterator, and source failures are not swallowed', async () => {
  const controller = new AbortController()
  controller.abort()
  const untouched = {
    [Symbol.asyncIterator]() {
      assert.fail('pre-abort must not acquire input')
    },
  }
  await assert.rejects(streamingSpeechSegments(untouched, controller.signal).next(), {
    name: 'AbortError',
  })
  const input = source()
  const output = streamingSpeechSegments(input, signal())
  const first = observe(output.next())
  await tick()
  input.fail(new Error('Source failed'))
  await first.done
  assert.match(first.error.message, /Source failed/)
  assert.equal(input.returned, 1)
})
