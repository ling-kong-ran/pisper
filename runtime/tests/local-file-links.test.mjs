import assert from 'node:assert/strict'
import test from 'node:test'
import {
  decodeLocalFileHref,
  encodeLocalFileHref,
  parseLocalFileTarget,
  remarkLocalFileLinks,
  remarkPreviewLinks,
} from '../../src/lib/local-file-links.ts'

const bases = ['C:\\work\\project', '/work/project', '/']

test('ordinary anchors, queries, and URL schemes remain unchanged by the remark plugin', () => {
  const links = [
    '#anchor',
    '?query=1',
    'app.ts#L12',
    'app.ts?download=1',
    '/docs/page#anchor',
    '/docs/page?query=1',
    'https://example.com/app.ts:12',
    'http://example.com/file',
    'http:12',
    'https:12:4',
    'javascript:alert(1)',
    'javascript:12',
    'javascript:12:4',
    'javascript%3Aalert(1)',
    'data:text/plain,hello',
    'data:12',
    'mailto:user@example.com',
    'tel:1234',
    'ftp://example.com/file',
    'vbscript:12',
  ]
  for (const cwd of bases) {
    const tree = { type: 'root', children: links.map((url) => ({ type: 'link', url })) }
    remarkLocalFileLinks(cwd)(tree)
    assert.deepEqual(
      tree.children.map((node) => node.url),
      links,
    )
    for (const link of links) assert.equal(parseLocalFileTarget(link, cwd), null, link)
  }
})

test('file URLs use only pathname while encoded query and fragment characters remain filenames', () => {
  for (const [link, expected] of [
    ['file:///tmp/report.txt?download=1#L99', { path: '/tmp/report.txt' }],
    ['file:///tmp/report.txt:12:4?line=99#L88', { path: '/tmp/report.txt', line: 12, column: 4 }],
    ['file:///C:/work/report.txt:12?download=1#L99', { path: 'C:/work/report.txt', line: 12 }],
    ['file://localhost/tmp/report.txt?#', { path: '/tmp/report.txt' }],
    ['file://server/share/report.txt?download=1#L99', { path: '//server/share/report.txt' }],
    [
      'file:///tmp/report%3Fquery%23anchor.txt?ignored#ignored',
      { path: '/tmp/report?query#anchor.txt' },
    ],
    ['file:///C:/work/My%20Report.txt#L99', { path: 'C:/work/My Report.txt' }],
  ]) {
    const target = parseLocalFileTarget(link)
    assert.deepEqual(target, expected, link)
    assert.deepEqual(decodeLocalFileHref(encodeLocalFileHref(target)), expected, link)
  }
})

test('relative source positions resolve before scheme detection on POSIX and Windows', () => {
  for (const [cwd, prefix] of [
    ['C:\\work\\project', 'C:\\work\\project\\'],
    ['C:/work/project/', 'C:/work/project/'],
    ['/work/project/', '/work/project/'],
    ['/', '/'],
    ['C:\\', 'C:\\'],
  ]) {
    for (const [link, filename, position] of [
      ['app.ts:12', 'app.ts', { line: 12 }],
      ['app.ts:12:4', 'app.ts', { line: 12, column: 4 }],
      ['./README:12', 'README', { line: 12 }],
      ['src/app.ts:12:4', 'src/app.ts', { line: 12, column: 4 }],
      ['My%20Report.txt:12', 'My Report.txt', { line: 12 }],
    ]) {
      assert.deepEqual(parseLocalFileTarget(link, cwd), { path: prefix + filename, ...position })
    }
  }
  assert.deepEqual(parseLocalFileTarget('report.txt', '/'), { path: '/report.txt' })
  assert.equal(parseLocalFileTarget('app.ts:12'), null)
  assert.equal(parseLocalFileTarget('app.ts:12', 'relative/cwd'), null)
})

test('encoded local paths decode once across Markdown parsing and sentinel round trips', () => {
  for (const [cwd, prefix] of [
    ['/work/project', '/work/project/'],
    ['C:\\work\\project', 'C:\\work\\project\\'],
  ]) {
    for (const [link, filename] of [
      ['report%2520name.txt', 'report%20name.txt'],
      ['literal%252Fsegment.txt', 'literal%2Fsegment.txt'],
      ['literal%255Csegment.txt', 'literal%5Csegment.txt'],
      ['literal%252e%252e.txt', 'literal%2e%2e.txt'],
      ['literal%2500.txt', 'literal%00.txt'],
      ['report%3Fquery%23anchor.txt', 'report?query#anchor.txt'],
      ['./report%3A12', 'report:12'],
      ['报告%20文件.txt', '报告 文件.txt'],
      ['100%done.txt', '100%done.txt'],
    ]) {
      const tree = { type: 'root', children: [{ type: 'link', url: link + ':7:2' }] }
      remarkLocalFileLinks(cwd)(tree)
      const expected = { path: prefix + filename, line: 7, column: 2 }
      const target = decodeLocalFileHref(tree.children[0].url)
      assert.deepEqual(target, expected, link)
      assert.deepEqual(decodeLocalFileHref(encodeLocalFileHref(target)), expected, link)
    }
  }
  for (const link of ['/tmp/report%2520name.txt', 'C:/work/report%2520name.txt']) {
    const expected = { path: link.replace('%2520', '%20') }
    for (const value of [link, 'file://' + (link.startsWith('/') ? '' : '/') + link]) {
      assert.deepEqual(
        decodeLocalFileHref(encodeLocalFileHref(parseLocalFileTarget(value))),
        expected,
      )
    }
  }
})

test('sentinel decoding preserves literal native path characters and source-like filenames', () => {
  for (const path of ['/tmp/file:12', '/tmp/ file%20?query#hash ', 'C:\\work\\file%2Fname:12']) {
    const target = { path, line: 8, column: 3 }
    assert.deepEqual(decodeLocalFileHref(encodeLocalFileHref(target)), target)
  }
  for (const path of ['javascript:alert(1)', 'relative/file.txt', '/tmp/bad\u0000.txt']) {
    assert.equal(decodeLocalFileHref(encodeLocalFileHref({ path })), null)
  }
  assert.equal(decodeLocalFileHref('https://example.com/reveal?path=%2Ftmp%2Ffile'), null)
})

test('relative traversal and decoded control characters remain rejected', () => {
  for (const cwd of bases) {
    for (const link of [
      '../outside.txt',
      '..\\outside.txt',
      'src/../outside.txt:12',
      '%2e%2e/outside.txt',
      'src%2f..%2foutside.txt',
      'src%5c..%5coutside.txt',
      'bad%00.txt',
      '/tmp/bad%0A.txt',
      'file:///tmp/bad%00.txt',
      'file://user:password@server/share/file',
    ]) {
      assert.equal(parseLocalFileTarget(link, cwd), null, link)
    }
  }
})

function previewLinks(children, cwd = 'D:/ziliao/xiao1') {
  const tree = { type: 'root', children: [{ type: 'paragraph', children }] }
  remarkPreviewLinks(cwd)(tree)
  return tree.children[0].children
}

const hrefPath = (node) => decodeLocalFileHref(node.url)?.path

test('HTML paths in inline code become preview links, whatever the shape of the path', () => {
  for (const [value, expected] of [
    ['D:/ziliao/xiao1/index.html', 'D:/ziliao/xiao1/index.html'],
    ['D:\\ziliao\\我的 文件\\报告.HTM', 'D:\\ziliao\\我的 文件\\报告.HTM'],
    ['/home/me/site/index.html', '/home/me/site/index.html'],
    ['index.html', 'D:/ziliao/xiao1/index.html'],
    ['./dist/page.html', 'D:/ziliao/xiao1/dist/page.html'],
    ['src/index.html:12', 'D:/ziliao/xiao1/src/index.html'],
  ]) {
    const child = { type: 'inlineCode', value }
    const [link] = previewLinks([child])
    assert.equal(link.type, 'link', value)
    assert.equal(hrefPath(link), expected)
    // 显示的还是原来的行内代码。
    assert.deepEqual(link.children, [child])
  }
})

test('inline code that is a web address links directly, so the preview provider can take it', () => {
  const [link] = previewLinks([{ type: 'inlineCode', value: 'http://localhost:5173/' }])
  assert.equal(link.type, 'link')
  assert.equal(link.url, 'http://localhost:5173/')
})

test('inline code that is not a previewable path stays code', () => {
  for (const value of [
    'report.pdf',
    'npm run build index.html',
    '<div class="a.html">',
    'a.html b.html',
    '../outside.html',
    'ftp://example.com/a.html',
    'javascript:alert(1).html',
    'index.html?x=1',
    'index.htmlx',
    '',
  ]) {
    const child = { type: 'inlineCode', value }
    assert.deepEqual(previewLinks([child]), [child], value)
  }
  // 相对路径没有工作区就无法定位。
  const child = { type: 'inlineCode', value: 'index.html' }
  assert.deepEqual(previewLinks([child], ''), [child])
})

test('HTML paths in prose stop at Chinese punctuation and keep the surrounding text', () => {
  const nodes = previewLinks([
    { type: 'text', value: '已在当前目录创建 D:/ziliao/xiao1/index.html，双击或打开 site/a.htm。' },
  ])
  assert.deepEqual(
    nodes.map((node) => node.type),
    ['text', 'link', 'text', 'link', 'text'],
  )
  assert.equal(nodes[0].value, '已在当前目录创建 ')
  assert.equal(hrefPath(nodes[1]), 'D:/ziliao/xiao1/index.html')
  assert.deepEqual(nodes[1].children, [{ type: 'text', value: 'D:/ziliao/xiao1/index.html' }])
  assert.equal(nodes[2].value, '，双击或打开 ')
  assert.equal(hrefPath(nodes[3]), 'D:/ziliao/xiao1/site/a.htm')
  assert.equal(nodes[4].value, '。')
})

test('prose paths end cleanly and Chinese words before a relative path are not swallowed', () => {
  const [before, link, after] = previewLinks([{ type: 'text', value: 'see /tmp/a/报告.html.' }])
  assert.equal(before.value, 'see ')
  assert.equal(hrefPath(link), '/tmp/a/报告.html')
  assert.equal(after.value, '.')
  for (const value of [
    '没有路径的一句话',
    'index.html.bak',
    'example.com/a.html',
    'mysite.html5',
    '../outside.html',
    // 中文紧贴相对文件名时猜不出边界，宁可不链接。
    '已生成报告.html',
  ]) {
    const child = { type: 'text', value }
    assert.deepEqual(previewLinks([child]), [child], value)
  }
})

test('existing links and code blocks are left alone, inside lists and tables too', () => {
  const code = { type: 'code', value: 'open index.html' }
  const link = {
    type: 'link',
    url: 'https://example.com',
    children: [{ type: 'text', value: 'a.html' }],
  }
  const tree = {
    type: 'root',
    children: [
      code,
      { type: 'paragraph', children: [link] },
      {
        type: 'list',
        children: [
          {
            type: 'listItem',
            children: [{ type: 'paragraph', children: [{ type: 'inlineCode', value: 'a.html' }] }],
          },
        ],
      },
    ],
  }
  remarkPreviewLinks('/work')(tree)
  assert.deepEqual(tree.children[0], { type: 'code', value: 'open index.html' })
  assert.equal(tree.children[1].children[0].url, 'https://example.com')
  assert.deepEqual(tree.children[1].children[0].children, [{ type: 'text', value: 'a.html' }])
  const nested = tree.children[2].children[0].children[0].children[0]
  assert.equal(hrefPath(nested), '/work/a.html')
})
