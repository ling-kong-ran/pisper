(() => {
  const selectorFor = element => {
    if (element.id) return `#${CSS.escape(element.id)}`;
    const name = element.getAttribute('name'); if (name) return `${element.tagName.toLowerCase()}[name=${JSON.stringify(name)}]`;
    const testId = element.getAttribute('data-testid'); if (testId) return `[data-testid=${JSON.stringify(testId)}]`;
    const aria = element.getAttribute('aria-label'); if (aria) return `${element.tagName.toLowerCase()}[aria-label=${JSON.stringify(aria)}]`;
    return element.tagName.toLowerCase();
  };
  const elements = [...document.querySelectorAll('a,button,input,textarea,select,[role="button"],[contenteditable="true"]')]
    .filter(element => { const style = getComputedStyle(element); const rect = element.getBoundingClientRect(); return style.visibility !== 'hidden' && style.display !== 'none' && rect.width > 0 && rect.height > 0; })
    .slice(0, 100).map(element => ({
      selector: selectorFor(element), tag: element.tagName.toLowerCase(), role: element.getAttribute('role') || '',
      text: String(element.innerText || element.value || element.getAttribute('aria-label') || element.getAttribute('placeholder') || '').replace(/\s+/g, ' ').trim().slice(0, 240),
      href: element.href || '', type: element.getAttribute('type') || '', disabled: Boolean(element.disabled),
    }));
  return { title: document.title, url: location.href, text: String(document.body?.innerText || '').slice(0, 20000), elements };
})()
