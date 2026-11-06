// Agent 接管右侧浏览器时在网页里执行的脚本。
// 原生视图盖在 Pisper 的界面之上，指针和光效只能画进网页。网页的 CSP 和 Trusted Types 会挡
// <style> 和 innerHTML，所以全部用 DOM 接口建元素、用 Web Animations 做动画，这两样都不受限。
// 脚本必须同步返回：WebView2 的 ExecuteScript 不等 Promise。

const OVERLAY_ID = 'pisper-agent-overlay'
const CURSOR_SIZE = 28
const ACCENT = '124, 92, 255'
const CLICKABLE =
  'a,button,input,textarea,select,summary,label,[role="button"],[role="link"],[role="tab"],[role="menuitem"],[contenteditable="true"]'

const HELPERS = `
const done = (value) => JSON.stringify({ ok: true, ...value });
const pageInfo = () => ({ url: location.href, title: document.title });
const isVisible = (element) => {
  const style = getComputedStyle(element);
  const rect = element.getBoundingClientRect();
  return style.visibility !== 'hidden' && style.display !== 'none' && rect.width > 0 && rect.height > 0;
};
const labelOf = (element) => String(element.innerText || element.value || element.getAttribute('aria-label') || element.getAttribute('placeholder') || '').replace(/\\s+/g, ' ').trim();
const find = (selector) => {
  if (selector.startsWith('text=')) {
    const wanted = selector.slice(5).replace(/^["']|["']$/g, '').trim().toLowerCase();
    const candidates = [...document.querySelectorAll(${JSON.stringify(CLICKABLE)})].filter(isVisible);
    return candidates.find((element) => labelOf(element).toLowerCase() === wanted) || candidates.find((element) => labelOf(element).toLowerCase().includes(wanted)) || null;
  }
  const matches = [...document.querySelectorAll(selector)];
  return matches.find(isVisible) || matches[0] || null;
};
const center = (element) => {
  const rect = element.getBoundingClientRect();
  return { x: rect.left + rect.width / 2, y: rect.top + rect.height / 2 };
};
const overlayRoot = () => document.getElementById(${JSON.stringify(OVERLAY_ID)})?.shadowRoot || null;
`

function pageScript(args: Record<string, unknown>, body: string) {
  return `(() => { try { const args = ${JSON.stringify(args)}; ${HELPERS} ${body} } catch (error) { return JSON.stringify({ ok: false, error: String(error && error.message || error) }); } })()`
}

export function loadStateScript() {
  return pageScript({}, `return done({ ready: document.readyState !== 'loading', ...pageInfo() });`)
}

export function inspectScript({ maxElements, maxText }: { maxElements: number; maxText: number }) {
  return pageScript(
    { maxElements, maxText },
    `
const unique = (selector) => { try { return document.querySelectorAll(selector).length === 1 } catch { return false } };
const selectorFor = (element) => {
  const tag = element.tagName.toLowerCase();
  const candidates = [
    element.id && '#' + CSS.escape(element.id),
    element.getAttribute('data-testid') && '[data-testid=' + JSON.stringify(element.getAttribute('data-testid')) + ']',
    element.getAttribute('name') && tag + '[name=' + JSON.stringify(element.getAttribute('name')) + ']',
    element.getAttribute('aria-label') && tag + '[aria-label=' + JSON.stringify(element.getAttribute('aria-label')) + ']',
  ].filter(Boolean);
  const direct = candidates.find(unique);
  if (direct) return direct;
  // 没有能唯一定位的属性就用层级路径，否则模型拿到的 "button" 只会点到第一个。
  const path = [];
  for (let node = element; node && node.nodeType === 1 && node !== document.documentElement; node = node.parentElement) {
    if (node.id && unique('#' + CSS.escape(node.id))) { path.unshift('#' + CSS.escape(node.id)); break; }
    const sameTag = node.parentElement ? [...node.parentElement.children].filter((child) => child.tagName === node.tagName) : [node];
    const name = node.tagName.toLowerCase();
    path.unshift(sameTag.length > 1 ? name + ':nth-of-type(' + (sameTag.indexOf(node) + 1) + ')' : name);
  }
  return path.join(' > ');
};
const elements = [...document.querySelectorAll(${JSON.stringify(CLICKABLE)})].filter(isVisible).slice(0, args.maxElements).map((element) => ({
  selector: selectorFor(element),
  tag: element.tagName.toLowerCase(),
  role: element.getAttribute('role') || '',
  text: labelOf(element).slice(0, 240),
  href: element.href || '',
  type: element.getAttribute('type') || '',
  disabled: Boolean(element.disabled),
}));
return done({ ...pageInfo(), text: String(document.body && document.body.innerText || '').slice(0, args.maxText), elements });
`,
  )
}

// 把目标滚进可视区并报告中心点。找不到时不算出错：页面可能还在渲染，由调用方重试。
export function locateScript(selector: string) {
  return pageScript(
    { selector },
    `
const element = find(args.selector);
if (!element || !isVisible(element)) return done({ found: false });
element.scrollIntoView({ block: 'center', inline: 'center', behavior: 'instant' });
return done({ found: true, ...center(element) });
`,
  )
}

export function clickScript(selector: string) {
  return pageScript(
    { selector },
    `
const element = find(args.selector);
if (!element) throw new Error('No element matches ' + args.selector);
const point = center(element);
const hit = document.elementFromPoint(point.x, point.y);
const target = hit && (element.contains(hit) || hit.contains(element)) ? hit : element;
const init = { bubbles: true, cancelable: true, composed: true, view: window, clientX: point.x, clientY: point.y, button: 0 };
const press = { ...init, buttons: 1 };
target.dispatchEvent(new PointerEvent('pointerover', init));
target.dispatchEvent(new MouseEvent('mouseover', init));
target.dispatchEvent(new PointerEvent('pointermove', init));
target.dispatchEvent(new MouseEvent('mousemove', init));
target.dispatchEvent(new PointerEvent('pointerdown', press));
target.dispatchEvent(new MouseEvent('mousedown', press));
if (typeof element.focus === 'function') element.focus({ preventScroll: true });
target.dispatchEvent(new PointerEvent('pointerup', init));
target.dispatchEvent(new MouseEvent('mouseup', init));
target.dispatchEvent(new MouseEvent('click', init));
return done({ ...pageInfo() });
`,
  )
}

export function typeScript(selector: string, text: string, submit: boolean) {
  return pageScript(
    { selector, text, submit },
    `
const element = find(args.selector);
if (!element) throw new Error('No element matches ' + args.selector);
if (element.disabled || element.readOnly) throw new Error('The element is not editable: ' + args.selector);
element.focus({ preventScroll: true });
const notify = (name) => element.dispatchEvent(new Event(name, { bubbles: true, composed: true }));
if (element.tagName === 'SELECT') {
  const wanted = args.text.trim().toLowerCase();
  const option = [...element.options].find((item) => item.value.toLowerCase() === wanted || item.text.trim().toLowerCase() === wanted);
  if (!option) throw new Error('No option matches ' + args.text);
  element.value = option.value;
  notify('input');
  notify('change');
} else if (element.tagName === 'INPUT' || element.tagName === 'TEXTAREA') {
  // insertText 走的是浏览器的编辑流程，React 这类受控输入框能感知到；不行再用原生 setter。
  if (typeof element.select === 'function') element.select();
  let inserted = false;
  try { inserted = args.text ? document.execCommand('insertText', false, args.text) : false; } catch {}
  if (!inserted || element.value !== args.text) {
    const prototype = element.tagName === 'TEXTAREA' ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
    Object.getOwnPropertyDescriptor(prototype, 'value').set.call(element, args.text);
    notify('input');
  }
  notify('change');
} else if (element.isContentEditable) {
  const selection = getSelection();
  const range = document.createRange();
  range.selectNodeContents(element);
  selection.removeAllRanges();
  selection.addRange(range);
  if (!document.execCommand('insertText', false, args.text)) {
    element.textContent = args.text;
    notify('input');
  }
} else {
  throw new Error('The element is not editable: ' + args.selector);
}
if (args.submit) {
  const key = { key: 'Enter', code: 'Enter', keyCode: 13, which: 13, bubbles: true, cancelable: true, composed: true };
  const allowed = element.dispatchEvent(new KeyboardEvent('keydown', key));
  element.dispatchEvent(new KeyboardEvent('keypress', key));
  element.dispatchEvent(new KeyboardEvent('keyup', key));
  if (allowed && element.form && typeof element.form.requestSubmit === 'function') element.form.requestSubmit();
}
return done({ ...pageInfo() });
`,
  )
}

// 指针和光效。每次都能重复调用：导航后是新文档，覆盖层要重新建。
export function ensureOverlayScript({ x, y, label }: { x: number; y: number; label: string }) {
  return pageScript(
    { x, y, label, size: CURSOR_SIZE, accent: ACCENT },
    `
if (document.getElementById(${JSON.stringify(OVERLAY_ID)})) return done({ created: false });
const parent = document.documentElement;
if (!parent) return done({ created: false });
const host = document.createElement('div');
host.id = ${JSON.stringify(OVERLAY_ID)};
host.setAttribute('aria-hidden', 'true');
host.style.cssText = 'all: initial !important; position: fixed !important; inset: 0 !important; z-index: 2147483647 !important; pointer-events: none !important; display: block !important;';
const root = host.attachShadow({ mode: 'open' });
const calm = matchMedia('(prefers-reduced-motion: reduce)').matches;
const accent = (alpha) => 'rgba(' + args.accent + ', ' + alpha + ')';
const box = (id, styles) => { const element = document.createElement('div'); element.id = id; Object.assign(element.style, { position: 'absolute', pointerEvents: 'none' }, styles); root.append(element); return element; };

const glow = box('glow', { inset: '0', boxShadow: 'inset 0 0 0 2px ' + accent(0.85) + ', inset 0 0 44px 6px ' + accent(0.32) });
const enter = host.animate([{ opacity: 0 }, { opacity: 1 }], { duration: 240, easing: 'ease-out' });
if (!calm) glow.animate([{ opacity: 1 }, { opacity: 0.45 }], { duration: 1400, direction: 'alternate', iterations: Infinity, easing: 'ease-in-out' });

const tag = box('tag', { left: '50%', bottom: '16px', transform: 'translateX(-50%)', padding: '5px 12px', borderRadius: '999px', font: '500 12px/1.4 -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif', color: '#fff', background: accent(0.92), boxShadow: '0 4px 16px ' + accent(0.4), whiteSpace: 'nowrap' });
tag.textContent = args.label;

const cursor = box('cursor', { left: '0', top: '0', width: args.size + 'px', height: args.size + 'px', transform: 'translate(' + args.x + 'px, ' + args.y + 'px)', filter: 'drop-shadow(0 2px 6px rgba(0, 0, 0, 0.35))' });
const halo = document.createElement('div');
Object.assign(halo.style, { position: 'absolute', left: '-10px', top: '-10px', width: '20px', height: '20px', borderRadius: '50%', background: accent(0.35) });
cursor.append(halo);
if (!calm) halo.animate([{ transform: 'scale(0.8)', opacity: 0.9 }, { transform: 'scale(1.5)', opacity: 0.25 }], { duration: 1100, direction: 'alternate', iterations: Infinity, easing: 'ease-in-out' });
const svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
svg.setAttribute('viewBox', '0 0 24 24');
svg.setAttribute('width', String(args.size));
svg.setAttribute('height', String(args.size));
svg.style.position = 'absolute';
svg.style.overflow = 'visible';
const arrow = document.createElementNS('http://www.w3.org/2000/svg', 'path');
arrow.setAttribute('d', 'M1 1 L1 18.5 L6 14 L9.4 21.5 L12.8 20 L9.4 12.8 L16 12.4 Z');
arrow.setAttribute('fill', 'rgb(' + args.accent + ')');
arrow.setAttribute('stroke', '#fff');
arrow.setAttribute('stroke-width', '1.6');
arrow.setAttribute('stroke-linejoin', 'round');
svg.append(arrow);
cursor.append(svg);
parent.append(host);
return done({ created: true });
`,
  )
}

export function moveCursorScript(x: number, y: number, durationMs: number) {
  return pageScript(
    { x, y, durationMs },
    `
const cursor = overlayRoot() && overlayRoot().getElementById('cursor');
if (!cursor) return done({});
const to = 'translate(' + args.x + 'px, ' + args.y + 'px)';
const from = getComputedStyle(cursor).transform;
cursor.style.transform = to;
if (!matchMedia('(prefers-reduced-motion: reduce)').matches) cursor.animate([{ transform: from }, { transform: to }], { duration: args.durationMs, easing: 'cubic-bezier(0.22, 0.8, 0.24, 1)' });
return done({});
`,
  )
}

export function clickEffectScript(x: number, y: number) {
  return pageScript(
    { x, y, accent: ACCENT },
    `
const root = overlayRoot();
if (!root) return done({});
const ring = document.createElement('div');
Object.assign(ring.style, { position: 'absolute', left: (args.x - 16) + 'px', top: (args.y - 16) + 'px', width: '32px', height: '32px', borderRadius: '50%', border: '2px solid rgb(' + args.accent + ')', boxSizing: 'border-box', pointerEvents: 'none' });
root.append(ring);
ring.animate([{ transform: 'scale(0.4)', opacity: 1 }, { transform: 'scale(1.6)', opacity: 0 }], { duration: 420, easing: 'ease-out' }).onfinish = () => ring.remove();
return done({});
`,
  )
}

// 截图前藏起来：模型要看的是网页本身。
export function overlayVisibleScript(visible: boolean) {
  return pageScript(
    { visible },
    `
const host = document.getElementById(${JSON.stringify(OVERLAY_ID)});
if (host) host.style.setProperty('display', args.visible ? 'block' : 'none', 'important');
return done({});
`,
  )
}

export function removeOverlayScript() {
  return pageScript(
    {},
    `
const host = document.getElementById(${JSON.stringify(OVERLAY_ID)});
if (!host) return done({});
host.animate([{ opacity: 1 }, { opacity: 0 }], { duration: 200, easing: 'ease-in' }).onfinish = () => host.remove();
return done({});
`,
  )
}

// 结果是网页里算出来的，不可信：只认对象。不同平台可能多包一层 JSON 字符串。
export function parseScriptResult(raw: string): Record<string, unknown> {
  let value: unknown = raw
  for (let depth = 0; depth < 2 && typeof value === 'string'; depth++) {
    try {
      value = JSON.parse(value)
    } catch {
      throw new Error('The page returned an unreadable result.')
    }
  }
  if (!value || typeof value !== 'object' || Array.isArray(value))
    throw new Error('The page returned an unreadable result.')
  const record = value as Record<string, unknown>
  if (record.ok !== true)
    throw new Error(String(record.error || 'The page script failed.').slice(0, 500))
  return record
}
