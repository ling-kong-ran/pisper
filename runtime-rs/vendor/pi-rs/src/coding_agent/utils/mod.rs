//! Upstream `coding-agent` src/utils leaf modules (slice W3.1).
//!
//! Provenance map (upstream file → submodule), upstream SHA256 at migration
//! time:
//!
//! | upstream                    | submodule            | sha256 (first 12) |
//! |-----------------------------|----------------------|-------------------|
//! | abort.ts                    | [`abort`]            | f3a3265a2fb2      |
//! | ansi.ts                     | [`ansi`]             | 65f53effde03      |
//! | json.ts                     | [`json`]             | 7ee95ee422ab      |
//! | paths.ts                    | [`paths`]            | a687cdb54b82      |
//! | mime.ts                     | [`mime`]             | 00893b8a5089      |
//! | frontmatter.ts              | [`frontmatter`]      | 99142d78b94e      |
//! | deprecation.ts              | [`deprecation`]      | 5d299ae61779      |
//! | child-process.ts            | [`child_process`]    | cd324f5b9a1a      |
//! | fs-watch.ts                 | [`fs_watch`]         | f40e22497371      |
//! | git.ts                      | [`git`]              | bff4590aefa9      |
//! | text.ts                     | [`text`]             | bb323f9607c4      |
//!
//! Support ports (not upstream files, vendored library behavior the slice
//! depends on):
//!
//! - [`node_path`]: node's `path` module (win32 + posix) as required by
//!   [`paths`].
//! - [`node_url`]: node's `fileURLToPath` as required by [`paths`].
//! - [`hosted_git_info`]: the npm `hosted-git-info` 9.0.3 `fromUrl` surface
//!   as required by [`git`].
//!
//! Later native slices add [`shell`], [`shell_config`], [`image_process`],
//! [`exif_orientation`], [`management_http`], [`tools_manager`], [`clipboard`]
//! (M4 native-clipboard subprocess route), and host [`locale`] collation. The
//! image processing and shell modules document their remaining boundaries. Not
//! yet migrated here: `open-browser.ts`, `photon.ts`, `syntax-highlight.ts`,
//! `html.ts`, `changelog.ts`, `version-check.ts`, `windows-self-update.ts`,
//! `pi-user-agent.ts`, `sleep.ts`, and `tool-result-images.ts`.
//!
//! Divergence disclosure for [`fs_watch`]: upstream builds on node's
//! `fs.watch` (OS events). The Rust port uses `std`-only directory/file
//! polling because the `notify` crate is not in `Cargo.lock` and new
//! dependencies are forbidden in this slice; see the module docs there.
//!
//! The deterministic pure functions were validated against oracle output
//! captured from the real upstream sources under node (see
//! `tests/fixtures/utils_oracle/`); the captured values live in [`oracle_data`]
//! and are pinned by the per-module tests.

pub mod abort;
pub mod ansi;
pub mod child_process;
pub mod clipboard;
pub mod deprecation;
pub mod frontmatter;
pub mod fs_watch;
pub mod git;
pub mod hosted_git_info;
pub mod json;
pub mod mime;
pub mod node_path;
pub mod node_url;
pub mod paths;
pub mod text;

#[cfg(test)]
pub mod oracle_data;

/// Detached child lifetime slice of upstream shell.ts.
pub mod shell;

pub mod image_process;

/// Upstream `exif-orientation.ts`: pure EXIF orientation parsing + RGBA
/// transform, oracle-pinned against `image_oracle.json`.
pub mod exif_orientation;

pub mod shell_config;

pub mod management_http;
pub mod tools_manager;

pub mod locale;
