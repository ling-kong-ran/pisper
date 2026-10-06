// 与上游 InjectedScript 同一作用域，保持 CSS、文本、role、xpath 与内部组合选择器的语义。
const injected = new (module.exports.InjectedScript())(globalThis, {
  isUnderTest: false, sdkLanguage: 'javascript', frameSeq: 1,
  testIdAttributeName: 'data-testid', stableRafCount: 1, browserName: 'chromium',
  shouldPrependErrorPrefix: false, isUtilityWorld: true, customEngines: [],
});
return {
  split(selectorText) {
    const selector = injected.parseSelector(selectorText);
    visitAllSelectorParts(selector, (part, nested) => {
      if (nested && part.name === 'internal:control' && part.body === 'enter-frame')
        throw new Error('Frame locators are not allowed inside composite locators');
    });
    const result = []; let chunk = { parts: [] }; let start = 0;
    for (let i = 0; i < selector.parts.length; ++i) {
      const part = selector.parts[i];
      if (part.name === 'internal:control' && part.body === 'enter-frame') {
        if (!chunk.parts.length) throw new Error('Selector cannot start with entering frame, select the iframe first');
        result.push(chunk); chunk = { parts: [] }; start = i + 1; continue;
      }
      if (selector.capture === i) chunk.capture = i - start;
      chunk.parts.push(part);
    }
    if (!chunk.parts.length) throw new Error(`Selector cannot end with entering frame, while parsing selector ${selectorText}`);
    result.push(chunk);
    if (typeof selector.capture === 'number' && typeof result[result.length - 1].capture !== 'number')
      throw new Error('Can not capture the selector before diving into the frame. Only use * after the last frame has been selected');
    return result.map(value => stringifySelector(value));
  },
  query(selector) { return injected.querySelector(injected.parseSelector(selector), document, false); },
  states(node, fill) { return injected.checkElementStates(node, fill ? ['visible', 'enabled', 'editable'] : ['visible', 'enabled', 'stable']); },
  fill(node, text) { return injected.fill(node, text); },
  focus(node) { return injected.focusNode(node, true); },
  scroll(node, align) { if (!node.isConnected) return 'error:notconnected'; if (align) node.scrollIntoView({block: align, inline: align}); return 'done'; },
  intercept(node, point) { return injected.setupHitTargetInterceptor(node, 'mouse', point, false); },
  stop(interceptor) { return interceptor.stop(); },
  style(node) { return injected.describeIFrameStyle(node); },
  hit(node, point) { return injected.expectHitTarget(point, node); },
  frame(node) { if (!node.isConnected) return 'error:notconnected'; if (!['IFRAME', 'FRAME'].includes(node.nodeName)) throw new Error(`Selector resolved to ${injected.previewNode(node)}, <iframe> was expected`); return 'done'; },
};
