# Pisper TUI layout specification

> Companion specification for the MCP-maintained `docs/tui.pen` design.

## Canvas

Create three top-level frames in one horizontal row. Keep every frame self-contained and name it exactly as below.

| Frame | Pencil name | Position | Size |
| --- | --- | ---: | ---: |
| A | `A — Conversation First` | `x: 0, y: 0` | `1360 × 900` |
| B | `B — Event Ledger` | `x: 1480, y: 0` | `1360 × 900` |
| C | `C — Slash Menu` | `x: 2960, y: 0` | `1360 × 900` |

Use no permanent sidebar, dashboard tiles, nested cards, gradients, shadows, translucent blur, illustrations, or window-chrome imitation. Each frame should read as a terminal application: continuous text surfaces, rules, gutters, line-oriented activity, keyboard focus, and dense status information.

## Shared visual system

### Color tokens

| Token | Hex | Usage |
| --- | --- | --- |
| `bg` | `#0B0E11` | terminal background |
| `surface` | `#11161B` | composer or transient overlay only |
| `surfaceRaised` | `#171D23` | selected row / focused command |
| `rule` | `#28313A` | 1 px separators and keylines |
| `text` | `#D7DEE7` | primary text |
| `muted` | `#7F8B99` | timestamps, metadata, inactive controls |
| `faint` | `#4E5A66` | tree connectors, idle ticks |
| `cyan` | `#61C8D4` | Agent identity and active focus |
| `green` | `#8CCF7E` | success, additions, completed tool calls |
| `amber` | `#E6B566` | thinking, running state, full-access mode |
| `red` | `#E06C75` | failures and destructive action only |
| `violet` | `#B7A0E8` | Subagent identity only |
| `blue` | `#7AA2D6` | file paths and user identity |

The palette must not be dominated by one hue. All body text on `bg` must meet WCAG AA contrast; `faint` is decorative only and must never carry required information.

### Typography

- Font family: `JetBrains Mono`; fallback `Cascadia Mono, SFMono-Regular, Consolas, monospace`.
- Body: `14 px / 22 px`, weight `400`, letter spacing `0`.
- Compact metadata: `12 px / 18 px`, weight `400`, letter spacing `0`.
- Frame title: `14 px / 20 px`, weight `600`, letter spacing `0`.
- Never scale font with viewport width. Never use text larger than `16 px`.
- Left-align all text. Use tabular numerals for time, duration, token, and line counts.

### Shared shell rules

- Outer frame fill: `bg`; no radius and no shadow.
- No persistent header or top bar. The transcript starts at the top edge and session/runtime metadata belongs in the bottom status line.
- An empty Chat session uses a centered two-tone terminal Pisper logo above the composer; established conversations do not repeat the logo.
- Bottom status line: `height 24`, `12 px` type. It may be integrated into the composer boundary; never float it in a card.
- A pending approval replaces the composer at full terminal width. It must not switch to a centered rail or move the status line horizontally.
- Bracketed pastes preserve the complete submitted text but render long or multiline content as one `[Pasted text · …]` token in the composer.
- `Ctrl+C` aborts an active or approval-blocked run; it exits the TUI only while idle.
- Icon buttons are `28 × 28`, square or `4 px` radius, with Lucide-style 14 px symbols when available. Tooltip labels: `Attach file`, `Run command`, `Stop run`, `Submit`.
- Focus: one `1 px cyan` keyline or one `2 px` left marker. Avoid glow.
- Scrollbars: `6 px` track, thumb `#36414C`; reserve width so content never shifts.

### Shared realistic scenario

All three frames depict the same session at different interaction moments, using these facts consistently:

- Session: `Fix flaky workspace permission test`
- Repository: `E:/code/pi-coder`
- Branch: `fix/mcp-lifecycle`
- Model: `gpt-5.6-sol`
- Permission: `workspace`
- User request: investigate an intermittent MCP lifecycle test failure, keep unrelated changes, delegate log analysis, then implement and verify the narrow fix.
- Agent activity: thinking, `read`, `grep`, `bash`, one background Subagent, one edit, targeted test verification.
- Status: `3/4 tasks`, one Subagent running or completed depending on the frame, context near `41%`.

## A — Conversation First

### Intent

A single uninterrupted transcript. Tool and Subagent activity appears inline at the exact point it occurred, but collapses to one or two lines after completion. This is the calmest reading layout and gives conversation the full width.

### Geometry

| Region | Bounds | Style |
| --- | --- | --- |
| Transcript viewport | `0,0,1360,708` | full-width conversation surface |
| Composer | `240,724,880,136` | `surface`, border `1 rule`, radius `6` |
| Bottom status | `240,868,880,20` | no fill |

Transcript content uses the full terminal width and starts at the left edge, with top padding only. User and Agent messages use a compact `3`-column marker gutter (`›` and `●`); Thinking, Tool, and Subagent activity use a subordinate `6`-column label gutter. Message blocks are appended from top to bottom and the viewport begins scrolling only after rendered content exceeds its available height; messages are not bottom-anchored. Message blocks are not enclosed by cards.

### Visible content, top to bottom

1. User message, `height 74`:
   - Marker at `x:0`: `›` in `blue`.
   - Content begins at column `3`: `The MCP lifecycle test still fails intermittently on Windows. Keep the existing runtime changes, delegate log analysis, and make the narrowest fix. Run the targeted tests when done.`
   - Metadata beneath: `10:42:08  ·  workspace` in `muted`.
2. Agent reasoning, expanded, `height 68`:
   - Label: `THINK` in `amber`.
   - First line: `I’ll inspect the failing test and current MCP service changes, then delegate the captured log comparison.`
   - Second line prefixed with `└`: `Scope stays inside the lifecycle path; unrelated worktree changes remain untouched.` in `muted`.
3. Inline tool group, `height 110`, left rule `2 px rule` at content x:
   - `✓ read   runtime/tests/mcp-service.test.mjs                 18 ms`
   - `✓ grep   "disconnect|dispose|restart" runtime/services     31 ms`
   - `✓ read   runtime/services/mcp-service.mjs                  12 ms`
   - Tool names use `cyan`, paths use `blue`, checks use `green`, durations use `muted`.
   - Only the selected second row shows a detail line: `└ 6 matches across 2 files` in `muted`.
4. Subagent event, `height 68`:
   - Label: `SUB` in `violet`.
   - Main line: `log-analysis  running  · comparing 3 failed CI traces`.
   - Detail: `└ isolated context · inherited workspace policy · 00:18`.
   - Running indicator is a single amber `●`; no avatar or card.
5. Agent response, `height 150`:
   - Marker: `●` in `cyan`; response text begins at column `3`.
   - Text: `The failure is a close/restart race. The stale transport can emit one final disconnect after the replacement client is already registered, which clears the new client entry.`
   - Blank line, then: `I’m guarding cleanup by transport identity, then I’ll run the MCP service test directly and the runtime regression set.`
   - A narrow code excerpt below with no container fill:
     `if (clients.get(serverId)?.transport === transport) clients.delete(serverId)`
6. Current activity line pinned immediately above the composer, `height 30`, aligned to the terminal's left edge without an independent centered rail:
   - `● editing  runtime/services/mcp-service.mjs` in `amber/text`.
   - Right aligned: `00:41` in `muted`.

### Composer

- Inner padding `14`; input area `852 × 76`.
- Placeholder/current draft: `Also verify reconnect does not duplicate listeners.`
- Bottom control row at composer y `824`: left `+  /`, right `3 queued  ·  Ctrl+Enter submit`, then a `28 × 28` send icon button.
- Focus keyline on the composer only. Long draft wraps at approximately 96 monospace characters; maximum visible four lines, then internal scroll.

### Compact status

`gpt-5.6-sol  ·  workspace  ·  fix/mcp-lifecycle  ·  41% ctx` left aligned; `3/4 tasks  ·  1 subagent  ·  UTF-8` right aligned.

### Differentiator

No separate activity region exists. On completion, tool groups collapse to the first row plus `3 tools · 61 ms`; activating the row expands them in place without changing transcript width.

## B — Event Ledger

### Intent

A chronological, audit-friendly transcript. Time and event identity are first-class in a fixed left gutter, while conversation, tools, thinking, and Subagents share one ledger stream. This is the densest variant and should feel like `git log` plus a live build trace.

### Geometry

| Region | Bounds | Style |
| --- | --- | --- |
| Column header | `0,0,1360,32` | bottom border `1 rule` |
| Ledger viewport | `0,32,1360,716` | continuous background |
| Composer | `0,748,1360,126` | top border `1 rule`, `surface` |
| Bottom status | `0,876,1360,24` | `bg` |

Ledger columns:

- Time gutter: `x:20..112`, right aligned.
- Event rail: `x:126`, `1 px rule` vertical line; event glyph centered on the rail.
- Event type: `x:150..246`, fixed width `96`.
- Main content: `x:260..1130`, width `870`.
- Duration/state: `x:1150..1328`, right aligned.

Column header labels: `TIME`, `TYPE`, `EVENT`, `STATE`. Use `12 px muted` uppercase. Do not box rows.

### Ledger rows

Rows may be `28`, `44`, or `66 px` high but all content aligns to a `22 px` text baseline. Use these exact visible entries:

```text
10:42:08.114  ◆  USER      Investigate the intermittent MCP lifecycle failure…       accepted
10:42:10.006  ◇  THINK     Scope: lifecycle path; preserve unrelated worktree changes  1.8 s
10:42:11.201  ├  READ      runtime/tests/mcp-service.test.mjs                            18 ms
10:42:12.037  ├  GREP      disconnect|dispose|restart · 6 matches                       31 ms
10:42:13.422  ├  READ      runtime/services/mcp-service.mjs                              12 ms
10:42:14.108  ├  SUB:01    log-analysis started · isolated workspace                    running
10:42:29.663  ◇  AGENT     Found a stale transport cleanup race. Applying identity…     streamed
10:42:31.044  ├  EDIT      runtime/services/mcp-service.mjs · +4 −1                      9 ms
10:42:32.510  ├  TEST      node --test runtime/tests/mcp-service.test.mjs                 running
10:42:39.824  └  SUB:01    2/3 traces show late disconnect after replacement            done
```

Expanded row details:

- The `THINK` row is `66 px`; second line at content x: `└ Delegate trace comparison; inspect service locally; run focused regressions.`
- The `AGENT` row is `66 px`; second line: `└ Cleanup must check the active transport identity before deleting the client.`
- The running `TEST` row is `44 px`; second line in `muted`: `└ 8 passed · current: stale disconnect preserves replacement client`.

Color encoding:

- `USER` and `◆`: `blue`.
- `THINK` and `◇`: `amber`.
- `AGENT`: `cyan`.
- Tool types and rail connectors: primary `text` / `faint`.
- `SUB:01`: `violet`.
- `done`, checks, pass count: `green`; `running`: `amber`.

A faint vertical rail visually connects events but never becomes a project timeline illustration. Date boundary at top reads `TODAY · 12 MAR 2026` in `muted` with a horizontal rule.

### Composer

Composer preserves the ledger alignment:

- Time gutter shows `NOW` in `cyan`.
- Rail shows a focused `◆` glyph.
- Type column shows `INPUT`.
- Main input at `x:260`, width `870`, text: `Also verify reconnect does not duplicate listeners.`
- Second line: `/ attach  ·  @ context  ·  ! shell` in `muted`.
- Right column shows `Ctrl+Enter ↵`; submit is an icon at `x:1300`.
- Input supports three visible lines, then scrolls. The fixed columns never resize based on content.

### Bottom status

`EVENTS 10  ·  LIVE 2  ·  ERRORS 0` at left; `workspace · fix/mcp-lifecycle` centered; `gpt-5.6-sol · ctx 41%` right.

### Differentiator

Everything is one event ledger with a millisecond timeline and stable columns. There are no chat bubbles, drawer, or overlay in its default state. Filtering uses `/` and alters visible rows without changing column widths.

## C — Slash Menu

### Intent

The base interface remains a nearly chrome-free conversation. Typing `/` at the start of the composer opens a transient completion menu directly above it. TUI does not expose `Ctrl+K` or a separate command-menu button.

### Geometry

| Region | Bounds | Style |
| --- | --- | --- |
| Transcript | `0,0,1360,714` | text column `780`, centered at `x:290` |
| Slash menu | `290,310,780,404` | `surface`, border `1 #3A4652`, radius `6` |
| Composer prompt | `290,730,780,88` | only top and bottom `1 rule` |
| Compact status | `290,842,780,20` | no fill |

The menu is anchored `16 px` above the composer. It has no page scrim and does not blur the conversation. The opaque menu surface may cover older transcript rows while open.

### Trigger and catalog

- Open only when the composer begins with `/`; text after `/` filters the visible candidates.
- Merge the current runtime tool catalog, enabled Skills, and TUI-native commands into one list.
- Do not synthesize unavailable tools or disabled Skills.
- Tool selection remains subject to the existing Agent execution mode, workspace boundary, and approval chain.
- Closing the menu leaves the composer draft unchanged.

### Personal ordering

Filter first, then sort matching candidates by:

1. Descending successful-use count for the current user.
2. Descending last-used timestamp.
3. Stable command name order.

Increment usage only after a candidate is confirmed, not when it is merely highlighted. Store usage metadata under the normal Pisper data root through the shared sidecar/runtime contract; do not introduce a TUI-only legacy path.

### Menu content

The first row contains a slash icon and the current query `/`. Do not show an alternate invocation shortcut or an `Esc` keycap.

The scope row uses inline labels `ALL`, `TOOLS`, `SKILLS`, and `COMMANDS`, followed by `6 matches`. Result rows are `46 px` high with a `32 px` type column, `500 px` command column, and right-aligned usage count. The visible frequency-ranked example is:

1. Tool: `/read` — `Read workspace files` — `42 uses`.
2. Skill: `/frontend-design` — `Build distinctive production UI` — `31 uses`.
3. Tool: `/grep` — `Search file contents` — `28 uses`.
4. Skill: `/prompt-cache-optimizer` — `Optimize reusable prompt context` — `19 uses`.
5. Tool: `/bash` — `Run a shell command` — `17 uses`.
6. Command: `/model` — `Switch active model` — `11 uses`.

The selected row uses `surfaceRaised` and a `2 px cyan` left marker. Tool, Skill, and native command types use `cyan`, `violet`, and `amber` respectively. The footer contains only catalog counts and `MOST USED`; it does not teach keyboard shortcuts.

### Composer

- Prefix `❯` in `cyan`; draft `/`.
- Right icons: attach and submit only.
- Metadata line: `6 matches · current runtime catalog`.
- The composer retains focus while the menu is open; selection updates the composer command rather than creating a separate prompt field.

### Compact status

`gpt-5.6-sol · fix/mcp-lifecycle` left; `3/4 tasks · 1 subagent · UTF-8` right.

### Differentiator

This variant exposes runtime capabilities through the same text entry point used for conversation. Closing the slash menu restores a pure transcript with no drawer, gutter, sidebar, scrim, or persistent command surface.

## Content and overflow rules

- Use explicit text boxes with fixed widths from each layout; never rely on unconstrained auto-width for body text.
- Wrap prose on spaces. Paths and commands truncate in the middle only when required, preserving filename or final command argument, for example `runtime/…/mcp-service.test.mjs`.
- Maximum visible transcript line length: A `96`, B `92`, C `86` monospace characters.
- Tool/event rows remain one line. Expanded output may use two additional lines and then show `… 4 more lines`.
- Labels, timestamps, durations, and status values must use fixed columns and must not push adjacent text.
- Composer controls have fixed positions. Draft text scrolls before reaching the controls.
- Empty Chat sessions center an `88`-column composer below the Pisper logo. The first submitted message switches to the conversation layout in the same frame, pins the composer to the bottom edge, and expands it to the full terminal width.
- Conversation messages align to the terminal's left edge and grow downward from the top of the transcript. Short conversations leave unused space below; only overflowing content scrolls.
- Reserve the final `16 px` at the right edge of every scrollable viewport for scrollbar and safety spacing.
- At `1360 × 900`, no text may touch or cross a region boundary. Every frame must show its composer and status without scrolling.

## Pencil construction order

1. Create the three named top-level frames at the exact canvas coordinates.
2. Apply the shared background and typography without adding persistent header chrome.
3. Build each layout from full-width separators and text groups; avoid reusable card components.
4. Add visible scenario content before decorative states so wrapping can be checked early.
5. Add fixed-width metadata columns, current inline activity state, composer, and status line.
6. For C, create the base transcript first, then add the composer-anchored slash menu as the final layer.
7. Name major groups `transcript`, `composer`, and `status`; in C also name `slash-menu`.

## Screenshot verification checklist

Capture one full-frame screenshot for each top-level frame at 100% zoom and one canvas overview showing A–C side by side.

- All frame names A–C are visible and unambiguous in the canvas overview.
- A reads as one conversation stream with inline activity.
- B reads as a chronological ledger with a stable time/event gutter.
- C reads as a minimal transcript with one composer-anchored slash menu.
- User, Agent, thinking, tool, Subagent, composer, and compact status information are visible in every frame.
- No frame contains a permanent sidebar, chat bubbles, metric cards, nested cards, large branding, or desktop dashboard navigation.
- No text is clipped, ellipsized unnecessarily, or overlapped at `1360 × 900`.
- Running, success, focus, Subagent, and failure colors remain distinguishable without making the interface colorful or noisy. Running activity uses a short fixed-width pulse at the bottom-left status line, never a marquee or persistent top row.
- Borders are crisp at 1 px; baseline rhythm remains aligned at 22 px.
- C shows `/` in the focused composer and frequency-ranked Tool, Skill, and Command matches; B running event is legible; A composer is visibly focused.

## Required MCP verification when available

After reconstruction with Pencil MCP, perform these checks through MCP rather than manual JSON inspection:

1. Read the Pencil document tree and confirm exactly three top-level frames with the specified names and sizes.
2. Inspect each frame for overflowing text nodes and out-of-bounds children.
3. Use the batch design editor for corrections; do not hand-edit `.pen` JSON.
4. Render screenshots of all three frames and the overview.
5. Re-inspect after corrections and confirm no overflow, overlap, or unintended persistent sidebar/card grid remains.
