# Agent image assets parity

The new module did not exist at task start. It is owned separately from the shared
image pipeline, main, provider configuration and tool gateway.

Read-only release oracle: `582160235671903d9f1c7034b457557b1df74b68`,
`runtime/tools/app/image-assets.mjs`, `runtime/services/image-tools-plugin.mjs`,
`image-agent-service.mjs`, `image-agent-media.mjs`, `image-agent-export.mjs` and their
behavior tests. No user profile or credentials are fixtures.

Read-only Rust dependency baseline (SHA-256):

- `image_nodes.rs`: `58c5a4592f6ab7aa8f5e1d1eab9f86760fc70a58bf36597092d8ca6143322bbf`
- `media.rs`: `8b7a6aa72b7682fcbf8f5c30efd93092dd92802f6d13059d329e4b3c82b36458`
- `image_protocol.rs`: `84761d467a3f7a14d05bf4d3504e0a3b1f60310930bdda2340a3a6d39edb22eb`

The tool JSON Schema is transcribed from the oracle's pure TypeBox declarations
(source SHA-256 `3572b5735bab849f21b897912553036eae632ccdbc02c50e9a3efa6c4cf6e998`,
formatted schema SHA-256 `7fa203d43a33633a34dcd8bce6494ec0870d59910b2cb8aabff8aa55d243e722`).
No Node service is used for production execution. All nine operations run
through native `ImageNodeService::operate`. Agent media is stored under
`data_dir/image-tools-agent`, separate from workflow and game media.

Native pixel algorithms and PNG/JPEG/WebP codecs are borrowed. The existing Tract
inference call cannot be interrupted within `model.run`; cancellation and shutdown
wait for the real CPU task. This is cooperative draining, not release's killable
Worker/120-second hard termination guarantee. That shared-pipeline gap remains.

Implemented boundaries:

- The inactive Pi extension reads actual execution cwd/session ID, validates the
  exact tool schema before import, and rechecks the injected canonical enable flag.
- Import permits only ordinary files below the canonical workspace root. It
  rejects child links/reparse points, URLs, 8 MiB overflow and over 4096 sides or
  16,000,000 pixels; native file/parent identities are rechecked around bounded reads.
- Export verifies stored atlas identity, PNG bytes, dimensions and frame rectangles;
  it creates fresh UUID directories and writes portable PNG/JSON without media IDs.
  Failure cleanup cannot remove a replacement directory or earlier exports.
- Owned tasks drain through actual media commits, operations, exports and generated
  file indexing. Indexing errors preserve successful file results. Closing this
  domain does not dispose shared pixel processing or Provider generation.

Sixteen scoped Rust behavior tests cover all nine operations, native PNG/JPEG/WebP
decode, media restart/isolation, strict arguments, workspace junctions, file/parent
replacement, size/pixel limits, safe cleanup, fresh enable checks, held authorization,
caller drop/cancel, committing import, pending export/operation/indexing shutdown and
public error redaction. The generation fixture writes real synthetic PNG pixels;
it is evidence for operation dispatch, not a paid Provider or full visual API.

Root owns boot/factory/configuration/asset indexing/shutdown integration. Source
`rustfmt --check` passed; compilation and these behavior tests are pending Root's
shared scoped run, followed by real Pi/HTTP attachment-archive acceptance. No claim
is made that capability flags or full release parity are already complete.
