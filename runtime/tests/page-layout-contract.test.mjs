import assert from 'node:assert/strict'
import test from 'node:test'
import { createElement as h } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { PageFrame } from '../../src/components/app/PageFrame.tsx'
import { PageState } from '../../src/components/app/PageState.tsx'
import { ListDetailLayout } from '../../src/components/app/ListDetailLayout.tsx'
import {
  FeatureList,
  FeaturePage,
  FeatureRow,
  FeatureSearch,
  FeatureSection,
  FeatureToolbar,
} from '../../src/components/app/FeaturePage.tsx'
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
    'app/FeaturePage.tsx',
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

// 功能页模板读 usePhoneShell（媒体查询）：服务端渲染时给一个最小的 window，按需模拟手机视口。
function renderWithViewport(phone, element) {
  const previous = globalThis.window
  globalThis.window = {
    matchMedia: (query) => ({
      matches: phone && /max-width/.test(query),
      addEventListener() {},
      removeEventListener() {},
    }),
  }
  try {
    return renderToStaticMarkup(element)
  } finally {
    if (previous === undefined) delete globalThis.window
    else globalThis.window = previous
  }
}

function featurePage({ rows = true, state } = {}) {
  return h(
    FeaturePage,
    {
      title: '插件',
      description: '管理代理可以调用的工具。',
      back: { label: '功能', ariaLabel: '返回功能', onBack() {} },
      toolbar: h(FeatureToolbar, {
        search: h(FeatureSearch, {
          id: 'feature-page-search',
          value: '',
          onValueChange() {},
          placeholder: '搜索插件',
          shortcut: '/',
          clearLabel: '清除搜索',
        }),
        actions: h('button', { type: 'button' }, '保存策略'),
      }),
    },
    state ??
      (rows &&
        h(
          FeatureSection,
          { title: '已安装', count: 2 },
          h(
            FeatureList,
            null,
            h(FeatureRow, {
              name: 'web_search',
              nameMeta: 'v1.4.2',
              description: '联网搜索',
              status: '本地',
              actions: h('button', { type: 'button', role: 'switch' }, ''),
              detail: h('p', null, '完整详情'),
              expanded: true,
              onExpandedChange() {},
              toggleLabel: '展开 web_search 的详情',
            }),
            h(FeatureRow, { name: 'read', description: '读取文件' }),
          ),
        )),
  )
}

test('feature page template: drag-only top bar, centered 760 column, back above the title, toolbar and list rows', () => {
  const html = renderWithViewport(false, featurePage())
  assert.match(html, /data-page-template="feature"/)
  // 顶栏 52 只作拖拽区，里面没有任何控件（Windows 窗口按钮的 144px 角天然空着）。
  assert.match(
    html,
    /<div data-window-drag-region="true" aria-hidden="true" class="h-\[52px\] flex-none"><\/div>/,
  )
  // 页面自己的滚动区与居中 760 栏（max-w 808 含左右 24 内边距）。
  assert.match(html, /data-feature-scroller="true" class="[^"]*overflow-y-auto/)
  assert.match(
    html,
    /data-feature-column="true" class="mx-auto flex w-full max-w-\[808px\][^"]*px-6/,
  )
  // “‹ 功能”在页标题上方：可见文字只写去处，读屏读出“返回功能”；h1 可见，说明紧随其后。
  assert.match(
    html,
    /<button type="button" data-back-to-more="true" aria-label="返回功能"[^>]*>.*?<span>功能<\/span><\/button><h1 class="(?![^"]*sr-only)[^"]*">插件<\/h1><p[^>]*>管理代理可以调用的工具。<\/p>/,
  )
  // 工具行：搜索框（外壳“/”认识的 id）+ 主操作。
  assert.match(html, /data-feature-toolbar="true"/)
  assert.match(html, /id="feature-page-search" type="search"[^>]*aria-keyshortcuts="\/"/)
  assert.match(html, /data-feature-actions="true"[^>]*><button type="button">保存策略<\/button>/)
  // 分区标题“已安装 · 2”，列表保留读屏语义，行是行而不是卡片。
  assert.match(html, /<h2 id="[^"]+" class="[^"]*">已安装<span> · 2<\/span><\/h2>/)
  assert.match(html, /<ul role="list" data-feature-list="true"/)
  assert.equal(html.match(/data-feature-row="true"/g)?.length, 2)
  // 可展开的行：名称区是带 aria-expanded / aria-controls 的按钮，详情是以行名命名的区域；箭头不进 Tab 顺序。
  assert.match(html, /data-feature-row-toggle="true" aria-expanded="true" aria-controls="([^"]+)"/)
  const controls = html.match(/aria-controls="([^"]+)"/)[1]
  assert.match(
    html,
    new RegExp(
      `id="${controls}" role="region" aria-labelledby="[^"]+" data-feature-row-detail="true"`,
    ),
  )
  assert.match(html, /tabindex="-1" aria-hidden="true" title="展开 web_search 的详情"/)
  // 不可展开的行没有按钮外壳。
  assert.equal(html.match(/data-feature-row-toggle/g)?.length, 1)
})

test('feature page template keeps loading, empty and error states inside the page with the title and toolbar', () => {
  for (const kind of ['loading', 'empty', 'error']) {
    const html = renderWithViewport(
      false,
      featurePage({
        state: h(PageState, {
          kind,
          title: '插件',
          description: '请重试',
          action: h('button', null, '重试'),
        }),
      }),
    )
    assert.match(html, /data-page-template="feature"/)
    assert.match(html, />插件<\/h1>/)
    assert.match(html, /data-feature-toolbar="true"/)
    assert.match(html, kind === 'error' ? /role="alert"/ : /role="status"/)
    assert.match(html, /<button>重试<\/button>/)
  }
})

test('feature page template on the phone shell: title only for screen readers, 44px back, no drag bar', () => {
  const html = renderWithViewport(true, featurePage())
  // 手机外壳（规格 7.1、7.4）：页名在外壳顶栏，这里的 h1 视觉隐藏但保留；说明省掉；没有 52 拖拽区。
  assert.doesNotMatch(html, /data-window-drag-region/)
  assert.match(html, /<h1 class="[^"]*sr-only">插件<\/h1>/)
  assert.doesNotMatch(html, /管理代理可以调用的工具。/)
  // “‹ 功能”保留为 44px 文字返回键（iOS 没有返回键）；工具行按钮、行内按钮与箭头在手机宽度抬到 44。
  assert.match(
    html,
    /data-back-to-more="true"[^>]*class="[^"]*max-\[650px\]:h-11 max-\[650px\]:min-w-11/,
  )
  assert.match(
    html,
    /data-feature-actions="true" class="[^"]*max-\[650px\]:\[&amp;&gt;\*\]:min-h-11/,
  )
  assert.match(html, /tabindex="-1"[^>]*class="[^"]*max-\[650px\]:size-11/)
  // 状态挪到名称下面一行，右侧只留操作与箭头（不挤掉名称、不横向滚动）。
  assert.match(
    html,
    /data-feature-row-status="true" class="[^"]*max-\[650px\]:col-start-1 max-\[650px\]:row-start-2/,
  )
})
