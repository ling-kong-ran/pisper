import assert from 'node:assert/strict'
import test from 'node:test'
import { createElement as h } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { PageFrame } from '../../src/components/app/PageFrame.tsx'
import { PageState } from '../../src/components/app/PageState.tsx'
import { ListDetailLayout } from '../../src/components/app/ListDetailLayout.tsx'
import { readFile } from 'node:fs/promises'

test('loading, empty and error retain the page container and accessible state actions', () => {
  for (const kind of ['loading', 'empty', 'error']) {
    const html = renderToStaticMarkup(
      h(
        PageFrame,
        { variant: 'settings' },
        h(PageState, {
          kind,
          title: '模型设置',
          description: '请重试',
          action: h('button', null, '重试'),
        }),
      ),
    )
    assert.match(html, /data-page-template="settings"/)
    assert.match(html, kind === 'error' ? /role="alert"/ : /role="status"/)
    assert.match(html, /<button>重试<\/button>/)
  }
})

test('shared primitives have no business ancestor selectors or feature imports', async () => {
  for (const file of [
    'ui/app-primitives.tsx',
    'app/PageFrame.tsx',
    'app/PageState.tsx',
    'app/ListDetailLayout.tsx',
    'app/PageToolbar.tsx',
  ]) {
    const source = await readFile(new URL(`../../src/components/${file}`, import.meta.url), 'utf8')
    assert.doesNotMatch(source, /from ['"]@\/features\//, file)
    assert.doesNotMatch(
      source,
      /\[\.(?:memory-layout|detail-stack|skill-scope-head|cli-settings-heading|selection-list|model-config-heading)/,
      file,
    )
  }
})

test('list/detail switching keeps both views mounted so selection and scroll can survive', () => {
  for (const selected of [true, false]) {
    const html = renderToStaticMarkup(
      h(ListDetailLayout, {
        selected,
        onBack() {},
        backLabel: '返回列表',
        list: h('input', { defaultValue: '过滤条件' }),
        detail: h('textarea', { defaultValue: '未保存内容' }),
      }),
    )
    assert.match(html, /过滤条件/)
    assert.match(html, /未保存内容/)
    assert.match(html, /返回列表/)
    assert.match(html, new RegExp(`data-selected="${selected}"`))
  }
})
