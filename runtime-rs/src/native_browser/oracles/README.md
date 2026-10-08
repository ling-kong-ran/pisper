# Browser contract oracles

`tool-schema.json` is the actual readonly release tool's TypeBox parameter schema, generated under Node v24.19.0 with installed locked dependencies. `node24-browser-contract.json` records180 source coercion cases and38 service/tool/lifecycle steps against the unchanged release BrowserAutomationService and genuine Playwright1.62.1 / installed Microsoft Edge.

The generator and full report are saved under `release/local-rust-build/native-browser-contract/`. Its loader resolves the reference worktree's bare imports through existing development dependencies; it does not modify the reference or restore Node in production.

Private coercion functions/statements are executed verbatim from release source in Node VM. Actual browser proof is separate: trusted click/fill/Enter, CSS/text/role/xpath/nth selectors, same/cross-origin frames, inspect hints, real PNG pixels/full-page geometry, requested-versus-actual viewport metadata, isolated storage, explicit close, and delegated deletion. The actual source lifecycle method is called with unrelated persistence/Agent seams controlled; browser operations remain genuine. All67 browser-owned process IDs observed through CDP are absent after cleanup.

The ten-minute idle contract is600000ms. Its reset/stale-timer behavior is exercised through the release timer-injection seam while owning a genuine browser; ten minutes of wall-clock waiting is not claimed. Native clock tests and production teardown proofs are separate.

UTF16 truncation is explicit. Real Playwright fill converts the5000-unit cut's lone high surrogate to U+FFFD in DOM; inspect's20000-unit slice retains raw high-surrogate unit55357 before outgoing response cleaning. The oracle records raw units separately and cleans string values for valid serde-compatible JSON. Tool text JSON may retain escaped surrogate units where native Value cannot; that representation boundary is not hidden.

The release has a reproduced launch race: close/dispose while real browser launch is gated can return before a late session insertion, leaving no idle timer. Those records describe release behavior; the generator explicitly disposes again so it does not leak. Any native race correction requires an explicit scope decision rather than labeling it exact equivalence.

`scripts/smoke-rust-browser.mjs` provides the production Pi gateway acceptance helper and restart/cleanup hooks. Its browser runs must be real `discover_tools`/`call_tool` executions on owned loopback pages, with trusted event receipts, exact reference inspect comparison, PNG/durable assets, primary-only child rejection and owned process reaping. This helper was syntax checked by its author; a complete native backend/Pi smoke pass belongs to Root integration and is not inferred from these Node oracles.
