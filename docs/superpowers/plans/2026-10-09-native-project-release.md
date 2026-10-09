# Native project picker and Rust release implementation plan

> **For agentic workers:** Use superpowers:executing-plans to implement the following tasks. Steps use checkbox syntax for tracking.

**Goal:** Use the operating system folder dialog for desktop project creation, remove Dependabot configuration, and publish the integrated Rust desktop release.

**Architecture:** Reuse the existing `pickSystemDirectory` desktop bridge from both sidebar entry points. Keep the existing directory browser for clients without a native bridge. Publish through the staged component workflow so it owns version metadata and tags.

**Tech Stack:** React, TypeScript, Tauri, Rust, GitHub Actions.

**Spec:** User request in this task: delete `.github/dependabot.yml`, use the operating system when creating a project folder, integrate Rust develop into release and publish.

## Global constraints

- Preserve existing integrated commits; no force pushes or new merge commits.
- Do not advance the release branch while a release workflow is active.
- Node 24; existing dependencies and platform bridge only.
- Workflow owns versions and tags. Rust desktop packaging currently supports Windows only.

## Review focus

Native selection cancellation, picker errors, repeated activation, component unmount during selection, and clients without a desktop bridge.

## Tasks

- [x] Verify the existing candidate is cancelled before advancing release.
- [x] Exercise both sidebar entry points with an isolated UI fixture; reproduce the missing native call.
- [x] Update `src/components/layout/SidebarRecentSessions.tsx` to share the native selection handler, prevent duplicate dialogs, and ignore results after unmount.
- [x] Delete `.github/dependabot.yml`.
- [x] Run the focused UI fixture, applicable frontend checks and a production build; obtain independent diff review.
- [x] Update the existing release entry to detect the Rust layout and dispatch its supported desktop channel; preserve branch/version checks and workflow ownership of metadata. Test Rust, legacy Node and rejection paths.
- [ ] Verify develop-rust is included, commit and push the fix, then run the supported staged desktop workflow for version 0.6.0.
- [ ] Inspect failure logs if necessary; verify the public release and assets before claiming publication. Upgrade the local installation with the released installer if not already current.

## Execution record

- Baseline: clean `merge/release-rust-0.6.0`, commit `2d2b2d2a` equals remote release. Rust develop already integrated.
- Existing run `37898226078` builds a candidate missing both requested changes; cancellation requested.
- Ruling: continue in the existing integration checkout and use the existing native bridge. The user has already authorized integration and publication.
- Existing workflow finished cancelled. UI regression observed before the fix: zero native calls, one custom directory dialog. After the fix, eight isolated Edge component interactions pass, including cancellation, retry, duplicate activation, errors, unmount and browser fallback.
- Typecheck, lint (existing warnings), i18n and production build passed. Release entry behavior tests: 11 passed. Independent review found no actionable issues in the product and release changes.
- Ruling: the release entry still dispatched deleted Node/SEA channels. Detect Rust layout in the existing entry, preserve the staged workflow, and include bundled TUI paths in Desktop ownership. Do not add unsupported platform releases.
- Run `37902332765` completed with one workbench UI failure before installer generation. The phone layout remounted its iframe; the mobile editing check now reopens the saved project and verifies opacity, duration and revision before continuing. The same failure was reproduced in an isolated focused runner, then the complete workbench flow passed in both worker and root verification. Earlier intermittent desktop pointer failures remain recorded; they were not changed or declared fixed.
- Runtime compilation was absent from the cache, and failed jobs did not save compiled targets. Add the actual Runtime workspace, preserve workspace crates and save on failure, while retaining every release gate. Upload only isolated diagnostic reports and screenshots under a separate artifact name excluded from installer publication.
- Run `37910965464` passed the workbench check but failed one visual model-switch request with a missing-key response. The vendored SDK exposed a shared empty provider registry during full rebuild. An in-memory multi-thread regression reproduced this false missing-auth result; complete provider sets are now published atomically with a registration revision, retrying stale compositions without overwriting concurrent mutations.
- Independent review caught cancellation wakers being invoked under the new registry lock. Four deterministic watchdog regressions first failed; cancellation notification and provider destruction now happen after unlock. Final integration results: 13 registry regressions and 7 existing MCP regressions passed. Full Runtime tests: 456 passed, 7 ignored. Scoped Clippy, formatting and independent review passed. The new integration target runs in both release quality and packaging gates.
- Actual API validation with the rebuilt backend passed five sequential session/model changes, retained credential hashes, sent no provider requests and cleaned its owned process/ports. The earlier eight-way baseline did not reproduce the missing-key response but had transport timeouts; that stress observation is retained and is not declared fixed by this narrower API check. Remote release validation remains required.
- Run `37922458844` passed Windows compilation, the complete usability gate, embedded WebView2 signature verification and artifact upload. Finalization then rejected the missing installer updater signature: the Rust packager had not merged canonical updater configuration and had allowed unsigned staging. Signed builds now merge that configuration and require `.sig` during staging. Four full-entry packaging cases first reproduced the missing flags, then passed alongside the original eleven release tests. A real isolated Tauri signing fixture verifies normal bytes, rejects tampering, rejects unsigned strict staging, and produces the exact installer/signature/manifest set. Existing component-update behavior is unchanged; this does not claim to restore desktop automatic NSIS updating.
