# Pisper TUI layout specification

> Fallback artifact: Pencil MCP is not available in this agent context. This specification is intended for direct reconstruction in `docs/tui.pen`; the `.pen` file was not edited.

## Canvas

Create four top-level frames in one horizontal row. Keep every frame self-contained and name it exactly as below.

| Frame | Pencil name | Position | Size |
| --- | --- | ---: | ---: |
| A | `A — Conversation First` | `x: 0, y: 0` | `1360 × 900` |
| B | `B — Activity Drawer` | `x: 1480, y: 0` | `1360 × 900` |
| C | `C — Event Ledger` | `x: 2960, y: 0` | `1360 × 900` |
| D | `D — Command Overlay` | `x: 4440, y: 0` | `1360 × 900` |

Use no permanent sidebar, dashboard tiles, nested cards, gradients, shadows, translucent blur, illustrations, or window-chrome imitation. Each frame should read as a terminal application: continuous text surfaces, rules, gutters, line-oriented activity, keyboard focus, and dense status information.

## Shared visual system

### Color tokens

| Token | Hex | Usage |
| --- | --- | --- |
| `bg` | `#0B0E11` | terminal background |
| `surface` | `#11161B` | composer, drawer, or overlay only |
| `surfaceRaised` | `#171D23` | selected row / focused command |
| `rule` | `#28313A` | 1 px separators and keylines |
| `text` | `#D7DEE7` | primary text |
| `muted` | `#7F8B99` | timestamps, metadata, inactive controls |
| `faint` | `#4E5A66` | tree connectors, idle ticks |
| `cyan` | `#61C8D4` | Agent identity and active focus |
| `green` | `#8CCF7E` | success, additions, completed tool calls |
| `amber` | `#E6B566` | thinking, running state, workspace mode |
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
- Top bar: `height 42`, horizontal padding `20`, bottom border `1 rule`.
- Top bar left: Pisper wordmark in `text`, then `/`, then session title. Do not place a large logo.
- Top bar right: model, permission, branch, and context; use inline text separated by ` · `, not pills.
- Bottom status line: `height 24`, `12 px` type. It may be integrated into the composer boundary; never float it in a card.
- Icon buttons are `28 × 28`, square or `4 px` radius, with Lucide-style 14 px symbols when available. Tooltip labels: `Attach file`, `Run command`, `Stop run`, `Submit`.
- Focus: one `1 px cyan` keyline or one `2 px` left marker. Avoid glow.
- Scrollbars: `6 px` track, thumb `#36414C`; reserve width so content never shifts.

### Shared realistic scenario

All four frames depict the same session at different interaction moments, using these facts consistently:

- Session: `Fix flaky workspace sandbox test`
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
| Top bar | `0,0,1360,42` | shared top bar |
| Transcript viewport | `0,42,1360,666` | max text column `880`, centered at `x:240` |
| Composer | `240,724,880,136` | `surface`, border `1 rule`, radius `6` |
| Bottom status | `240,868,880,20` | no fill |

Transcript content uses `x:240`, width `880`, top padding `28`, bottom padding `24`. Keep labels in a fixed `76 px` column and content in an `804 px` column. Message blocks are separated by `22 px`, not enclosed by cards.

### Visible content, top to bottom

1. User message, `height 74`:
   - Label at `x:240`: `YOU` in `blue`, `12 px semibold`.
   - Content at `x:316`: `The MCP lifecycle test still fails intermittently on Windows. Keep the existing runtime changes, delegate log analysis, and make the narrowest fix. Run the targeted tests when done.`
   - Metadata beneath: `10:42:08  ·  workspace` in `muted`.
2. Agent reasoning, expanded, `height 68`:
   - Label: `THINK` in `amber`.
   - First line: `I’ll inspect the failing test and current MCP service changes, then delegate the captured log comparison.`
   - Second line prefixed with `└`: `Scope stays inside the lifecycle path; unrelated worktree changes remain untouched.` in `muted`.
3. Inline tool group, `height 110`, left rule `2 px rule` at content x:
   - `✓ read   server/tests/mcp-service.test.mjs                 18 ms`
   - `✓ grep   "disconnect|dispose|restart" server/services     31 ms`
   - `✓ read   server/services/mcp-service.mjs                  12 ms`
   - Tool names use `cyan`, paths use `blue`, checks use `green`, durations use `muted`.
   - Only the selected second row shows a detail line: `└ 6 matches across 2 files` in `muted`.
4. Subagent event, `height 68`:
   - Label: `SUB` in `violet`.
   - Main line: `log-analysis  running  · comparing 3 failed CI traces`.
   - Detail: `└ isolated context · inherited workspace sandbox · 00:18`.
   - Running indicator is a single amber `●`; no avatar or card.
5. Agent response, `height 150`:
   - Label: `AGENT` in `cyan`.
   - Text: `The failure is a close/restart race. The stale transport can emit one final disconnect after the replacement client is already registered, which clears the new client entry.`
   - Blank line, then: `I’m guarding cleanup by transport identity, then I’ll run the MCP service test directly and the runtime regression set.`
   - A narrow code excerpt below with no container fill:
     `if (clients.get(serverId)?.transport === transport) clients.delete(serverId)`
6. Current activity line pinned immediately above the composer, `height 30`:
   - `● editing  server/services/mcp-service.mjs` in `amber/text`.
   - Right aligned: `00:41` in `muted`.

### Composer

- Inner padding `14`; input area `852 × 76`.
- Placeholder/current draft: `Also verify reconnect does not duplicate listeners.`
- Bottom control row at composer y `824`: left `+  @  /`, right `3 queued  ·  Ctrl+Enter submit`, then a `28 × 28` send icon button.
- Focus keyline on the composer only. Long draft wraps at approximately 96 monospace characters; maximum visible four lines, then internal scroll.

### Compact status

`gpt-5.6-sol  ·  workspace  ·  fix/mcp-lifecycle  ·  41% ctx` left aligned; `3/4 tasks  ·  1 subagent  ·  UTF-8` right aligned.

### Differentiator

No separate activity region exists. On completion, tool groups collapse to the first row plus `3 tools · 61 ms`; activating the row expands them in place without changing transcript width.

## B — Activity Drawer

### Intent

Conversation remains a single stream, while live operational detail occupies a resizable bottom drawer. The drawer is temporary and run-scoped, resembling a terminal multiplexer pane rather than a dashboard.

### Geometry

| Region | Bounds | Style |
| --- | --- | --- |
| Top bar | `0,0,1360,42` | shared top bar |
| Transcript | `0,42,1360,450` | max text column `920`, centered at `x:220` |
| Drawer tab line | `0,492,1360,34` | top and bottom `1 rule` |
| Activity drawer | `0,526,1360,236` | `surface`, no radius |
| Composer | `220,778,920,82` | `bg`, top border `1 rule` |
| Bottom status | `0,876,1360,24` | `surface` |

A `4 px` horizontal resize handle spans `x:620..740` above the drawer tab line. The transcript reserves scrollbar width at right.

### Transcript content

Use speaker labels in a `64 px` leading column and a content width of `856 px`.

1. `YOU  10:42` followed by the complete user request from the shared scenario.
2. `AGENT  10:42` followed by: `I’ll isolate the race first. I’ve started a log-analysis Subagent while I inspect the service and test locally.`
3. One subdued run marker: `──────── run #184 · started 10:42:11 ────────`.
4. Current Agent response: `The stale disconnect callback is not scoped to the transport that installed it. I’m applying an identity guard and validating both restart and explicit disconnect behavior.`

The transcript shows no individual tool rows while the drawer is open; its only live cue is an amber `● running` after `AGENT`.

### Drawer tab line

Left at `x:20`:

- Active tab: `ACTIVITY  6` with `cyan` text and a `2 px cyan` underline.
- Inactive tabs: `CHANGES  1`, `PROBLEMS  0`, `TERMINAL` in `muted`.

Right at `x:1210`: collapse chevron icon, `Esc` hint, close icon. Use icons, not rounded text buttons.

### Activity drawer content

Use terminal columns: `time 78 px`, `state 24 px`, `operation 610 px`, `target 430 px`, `duration 90 px`. Header row at y `536`, body rows `26 px` high.

Visible rows:

```text
10:42:11  ✓  read       server/tests/mcp-service.test.mjs                 18 ms
10:42:12  ✓  grep       disconnect|dispose|restart · 6 matches            31 ms
10:42:13  ✓  read       server/services/mcp-service.mjs                   12 ms
10:42:14  ●  subagent   log-analysis · comparing 3 failed CI traces       00:27
10:42:31  ✓  edit       server/services/mcp-service.mjs · +4 −1            9 ms
10:42:32  ●  bash       node --test server/tests/mcp-service.test.mjs      00:09
```

- Selected row is the running `bash` row with `surfaceRaised` fill and a `2 px cyan` left marker.
- Expand selected row into the remaining drawer height, showing two indented output lines:
  - `TAP version 13`
  - `# Subtest: disconnect from stale transport preserves replacement client`
- Running symbols pulse only by opacity, no movement or layout shift.
- A Subagent completion should turn `●` into `✓` and append `· found late disconnect in 2/3 traces`; it must not create a popup.

### Composer

Single compact terminal prompt line:

- Prefix `❯` in `cyan` at `x:220`.
- Draft at `x:244`: `Also verify reconnect does not duplicate listeners.`
- Right controls: attachment icon, stop square while running, submit arrow.
- A second metadata line inside the composer: `queued after current run  ·  Enter newline  ·  Ctrl+Enter send` in `muted`.

### Bottom status

Left: `Pisper  ·  fix/mcp-lifecycle  ·  workspace`; center: `run #184  00:30`; right: `gpt-5.6-sol  ·  ctx 41%  ·  3/4 tasks`.

### Differentiator

The operational stream is spatially separate but only along the bottom. Closing the drawer returns its `270 px` to conversation; the `ACTIVITY 6` tab becomes a one-line footer indicator.

## C — Event Ledger

### Intent

A chronological, audit-friendly transcript. Time and event identity are first-class in a fixed left gutter, while conversation, tools, thinking, and Subagents share one ledger stream. This is the densest variant and should feel like `git log` plus a live build trace.

### Geometry

| Region | Bounds | Style |
| --- | --- | --- |
| Top bar | `0,0,1360,42` | shared top bar |
| Column header | `0,42,1360,32` | bottom border `1 rule` |
| Ledger viewport | `0,74,1360,674` | continuous background |
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
10:42:11.201  ├  READ      server/tests/mcp-service.test.mjs                            18 ms
10:42:12.037  ├  GREP      disconnect|dispose|restart · 6 matches                       31 ms
10:42:13.422  ├  READ      server/services/mcp-service.mjs                              12 ms
10:42:14.108  ├  SUB:01    log-analysis started · isolated workspace                    running
10:42:29.663  ◇  AGENT     Found a stale transport cleanup race. Applying identity…     streamed
10:42:31.044  ├  EDIT      server/services/mcp-service.mjs · +4 −1                      9 ms
10:42:32.510  ├  TEST      node --test server/tests/mcp-service.test.mjs                 running
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

## D — Command Overlay

### Intent

The base interface is a nearly chrome-free conversation. A keyboard command panel overlays the lower-middle canvas for fast navigation and execution. Activity is summarized in one inline run line unless explicitly opened through the command panel.

### Geometry

| Region | Bounds | Style |
| --- | --- | --- |
| Minimal top line | `0,0,1360,32` | no fill, bottom border `1 rule` |
| Transcript | `0,32,1360,682` | text column `780`, centered at `x:290` |
| Composer prompt | `290,730,780,88` | only top and bottom `1 rule` |
| Compact status | `290,842,780,20` | no fill |
| Overlay scrim | `0,32,1360,682` | `#0B0E11` at `36%` opacity |
| Command panel | `290,184,780,404` | `surface`, border `1 #3A4652`, radius `6` |

The top line contains only `pisper / Fix flaky workspace sandbox test` left and `workspace · 41%` right. Branch and model move to the status line.

### Base transcript behind overlay

Keep enough content visible around the panel edges to establish the conversation:

1. User text at y `78`: complete shared user request, no speaker block; prefix `you ›` in `blue`.
2. Agent response at y `176`: prefix `agent ›` in `cyan`; text starts `I isolated the failure to stale transport cleanup and delegated trace comparison.`
3. One compact activity line at y `284`: `↳ 3 tools complete · log-analysis running · test running 00:09` with `amber` running state.
4. Agent continuation below the panel at y `624`: `The identity guard is in place. The focused MCP service test is still running.`

The scrim dims content but does not blur it.

### Command panel

#### Search row

- Bounds `306,200,748,48`; bottom border `1 rule`.
- Leading search icon `16 px` at x `322`.
- Input text at x `350`: `run test`.
- Right hint: `Esc` in `muted`; keycap is plain text with a `1 px rule` outline and `3 px` radius.

#### Scope row

Bounds `306,248,748,34`. Inline segmented text, not pill buttons:

- `ALL` active in `cyan` with `2 px` underline.
- `COMMANDS`, `FILES`, `SESSIONS` inactive in `muted`.
- Right aligned result count: `6 results`.

#### Results

Rows are `46 px` high, x `306`, width `748`. Icon column `32`, command column `500`, shortcut column `184`. No card per result.

1. Selected, `surfaceRaised`, `2 px cyan` left marker:
   - Icon: terminal prompt symbol.
   - Primary: `Run focused MCP service tests`.
   - Secondary: `node --test server/tests/mcp-service.test.mjs`.
   - Shortcut: `Enter`.
2. Primary: `Show current run activity`; secondary: `6 events · 2 live`; shortcut: `Ctrl+J`.
3. Primary: `Open changed file`; secondary: `server/services/mcp-service.mjs`; shortcut: none.
4. Primary: `Message Subagent log-analysis`; secondary: `running · 00:27`; shortcut: `Ctrl+Shift+A`.
5. Primary: `Stop current run`; secondary: `run #184`; icon and text use `red`; shortcut: `Ctrl+C`.
6. Primary: `Switch execution mode`; secondary: `workspace`; shortcut: none.

Footer at y `556`, height `32`: `↑↓ navigate  ·  Enter run  ·  Tab preview` left; `> commands` right. Both use `12 px muted`.

The selected result has no rounded pill treatment. Only the command panel itself is framed. A preview, if opened with `Tab`, replaces the result list rather than adding a second card.

### Composer behind overlay

- Prefix `❯` in `cyan`; draft `Also verify reconnect does not duplicate listeners.`
- Right icons: attach, command (`⌘`/terminal icon), submit.
- When the overlay is open, composer caret is hidden and focus belongs to search.

### Compact status

`gpt-5.6-sol · fix/mcp-lifecycle` left; `3/4 tasks · 1 subagent · UTF-8` right.

### Differentiator

This variant minimizes persistent UI and exposes tools, files, activity, modes, and Subagent actions through a transient command surface. Closing it restores an almost pure transcript with no drawer, gutter, sidebar, or card grid.

## Content and overflow rules

- Use explicit text boxes with fixed widths from each layout; never rely on unconstrained auto-width for body text.
- Wrap prose on spaces. Paths and commands truncate in the middle only when required, preserving filename or final command argument, for example `server/…/mcp-service.test.mjs`.
- Maximum visible transcript line length: A `96`, B `100`, C `92`, D `86` monospace characters.
- Tool/event rows remain one line. Expanded output may use two additional lines and then show `… 4 more lines`.
- Labels, timestamps, durations, and status values must use fixed columns and must not push adjacent text.
- Composer controls have fixed positions. Draft text scrolls before reaching the controls.
- Reserve the final `16 px` at the right edge of every scrollable viewport for scrollbar and safety spacing.
- At `1360 × 900`, no text may touch or cross a region boundary. Every frame must show its composer and status without scrolling.

## Pencil construction order

1. Create the four named top-level frames at the exact canvas coordinates.
2. Apply shared background, typography, and top bar primitives.
3. Build each layout from full-width separators and text groups; avoid reusable card components.
4. Add visible scenario content before decorative states so wrapping can be checked early.
5. Add fixed-width metadata columns, current activity state, composer, and status line.
6. For D, create the base transcript first, then add scrim and command panel as the final two layers.
7. Name major groups `topbar`, `transcript`, `activity`, `composer`, `status`; in D also name `command-overlay`.

## Screenshot verification checklist

Capture one full-frame screenshot for each top-level frame at 100% zoom and one canvas overview showing A–D side by side.

- All frame names A–D are visible and unambiguous in the canvas overview.
- A reads as one conversation stream with inline activity.
- B reads as a conversation plus a bottom terminal drawer, not a lower dashboard.
- C reads as a chronological ledger with a stable time/event gutter.
- D reads as a minimal transcript with one keyboard command overlay.
- User, Agent, thinking, tool, Subagent, composer, and compact status information are visible in every frame.
- No frame contains a permanent sidebar, chat bubbles, metric cards, nested cards, large branding, or desktop dashboard navigation.
- No text is clipped, ellipsized unnecessarily, or overlapped at `1360 × 900`.
- Running, success, focus, Subagent, and failure colors remain distinguishable without making the interface colorful or noisy.
- Borders are crisp at 1 px; baseline rhythm remains aligned at 22 px.
- D overlay search is visibly focused; B test row is visibly selected; C running event is legible; A composer is visibly focused.

## Required MCP verification when available

After reconstruction with Pencil MCP, perform these checks through MCP rather than manual JSON inspection:

1. Read the Pencil document tree and confirm exactly four top-level frames with the specified names and sizes.
2. Inspect each frame for overflowing text nodes and out-of-bounds children.
3. Use the batch design editor for corrections; do not hand-edit `.pen` JSON.
4. Render screenshots of all four frames and the overview.
5. Re-inspect after corrections and confirm no overflow, overlap, or unintended persistent sidebar/card grid remains.
