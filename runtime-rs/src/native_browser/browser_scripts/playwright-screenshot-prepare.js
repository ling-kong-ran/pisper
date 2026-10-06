function inPagePrepareForScreenshots(screenshotStyle, hideCaret, disableAnimations, syncAnimations) {
  if (syncAnimations) {
    const style = document.createElement("style");
    style.textContent = "body {}";
    document.head.appendChild(style);
    document.documentElement.getBoundingClientRect();
    style.remove();
  }
  if (!screenshotStyle && !hideCaret && !disableAnimations)
    return;
  const collectRoots = (root, roots2 = []) => {
    roots2.push(root);
    const walker = document.createTreeWalker(root, NodeFilter.SHOW_ELEMENT);
    do {
      const node = walker.currentNode;
      const shadowRoot = node instanceof Element ? node.shadowRoot : null;
      if (shadowRoot)
        collectRoots(shadowRoot, roots2);
    } while (walker.nextNode());
    return roots2;
  };
  const roots = collectRoots(document);
  const cleanupCallbacks = [];
  if (screenshotStyle) {
    for (const root of roots) {
      const styleTag = document.createElement("style");
      styleTag.textContent = screenshotStyle;
      if (root === document)
        document.documentElement.append(styleTag);
      else
        root.append(styleTag);
      cleanupCallbacks.push(() => {
        styleTag.remove();
      });
    }
  }
  if (hideCaret) {
    const elements = /* @__PURE__ */ new Map();
    for (const root of roots) {
      root.querySelectorAll("input,textarea,[contenteditable]").forEach((element3) => {
        elements.set(element3, {
          value: element3.style.getPropertyValue("caret-color"),
          priority: element3.style.getPropertyPriority("caret-color")
        });
        element3.style.setProperty("caret-color", "transparent", "important");
      });
    }
    cleanupCallbacks.push(() => {
      for (const [element3, value2] of elements)
        element3.style.setProperty("caret-color", value2.value, value2.priority);
    });
  }
  if (disableAnimations) {
    const infiniteAnimationsToResume = /* @__PURE__ */ new Set();
    const handleAnimations = (root) => {
      for (const animation of root.getAnimations()) {
        if (!animation.effect || animation.playbackRate === 0 || infiniteAnimationsToResume.has(animation))
          continue;
        const endTime = animation.effect.getComputedTiming().endTime;
        if (Number.isFinite(endTime)) {
          try {
            animation.finish();
          } catch (e) {
          }
        } else {
          try {
            animation.cancel();
            infiniteAnimationsToResume.add(animation);
          } catch (e) {
          }
        }
      }
    };
    for (const root of roots) {
      const handleRootAnimations = handleAnimations.bind(null, root);
      handleRootAnimations();
      root.addEventListener("transitionrun", handleRootAnimations);
      root.addEventListener("animationstart", handleRootAnimations);
      cleanupCallbacks.push(() => {
        root.removeEventListener("transitionrun", handleRootAnimations);
        root.removeEventListener("animationstart", handleRootAnimations);
      });
    }
    cleanupCallbacks.push(() => {
      for (const animation of infiniteAnimationsToResume) {
        try {
          animation.play();
        } catch (e) {
        }
      }
    });
  }
  window.__pwCleanupScreenshot = () => {
    for (const cleanupCallback of cleanupCallbacks)
      cleanupCallback();
    delete window.__pwCleanupScreenshot;
  };
}
