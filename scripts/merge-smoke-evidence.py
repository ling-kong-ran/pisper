# -*- coding: utf-8 -*-
"""Merge smoke-rust-usability report checks into the parity evidence file.

Maps each passed domain check to the release-normalized METHOD /path keys it
exercised. Conservative: only endpoints the check names clearly cover.
"""
import io, json, sys

report_path = sys.argv[1] if len(sys.argv) > 1 else \
    r'release/local-rust-build/usability/2026-10-06T06-05-28.705Z/report.json'
evidence_path = 'docs/reports/rust-release-parity-behavior-evidence.json'

report = json.load(io.open(report_path, encoding='utf-8'))
passed = [c['name'] for c in report['checks'] if c['ok']]

def has(*needles):
    return any(all(n.lower() in name.lower() for n in needles) for name in passed)

# check-name fragment(s) → evidence string; key list maps to ledger rows
M = []  # (fragments tuple, [endpoints], evidence text)

S = '/api/sessions/{sessionId}'
M.append((['MCP dashboard contract'], ['GET /api/mcp'], 'smoke: MCP dashboard contract'))
M.append((['MCP add preserves unknown config'], ['POST /api/mcp'], 'smoke: MCP add preserves unknown config and exposes real native tools'))
M.append((['MCP exact tool permission reload'], ['PATCH /api/mcp/{serverId}'], 'smoke: MCP exact tool permission reload preserves config and real tool catalog'))
M.append((['MCP connection test performs actual initialize'], ['POST /api/mcp/{serverId}/test'], 'smoke: MCP connection test performs actual initialize/list and redacts secrets'))
M.append((['MCP server disable/enable reload'], ['PATCH /api/mcp/{serverId}/tools/{toolName}'], 'smoke: MCP server disable/enable reload actually unregisters/registers tools'))
M.append((['MCP delete removes config'], ['DELETE /api/mcp/{serverId}'], 'smoke: MCP delete removes config and native tools, preserving unknown root fields'))
M.append((['Provider discovery contract'], ['GET /api/providers/discovery'], 'smoke: Provider discovery contract'))
M.append((['Discover a connection against local /v1/models'], ['POST /api/providers/models/discover-connection'], 'smoke: discover a connection against local /v1/models'))
M.append((['Create and save a usable provider connection'], ['POST /api/providers', 'PUT /api/providers/{providerId}/connection', 'PUT /api/providers/{providerId}/api-key'], 'smoke: create and save a usable provider connection'))
M.append((['Save default provider/model through React PUT contract'], ['PUT /api/config'], 'smoke: save default provider/model through React PUT contract'))
M.append((['Frontend config schema'], ['GET /api/config'], 'smoke: frontend config schema'))
M.append((['Create a real session with frontend summary'], ['POST /api/sessions', 'GET /api/sessions'], 'smoke: create a real session with frontend summary'))
M.append((['Live session is JSON with usable messages'], [f'GET {S}/live', f'GET {S}/messages'], 'smoke: live session JSON with usable messages/pageInfo'))
M.append((['Plain chat produces real streamed assistant text'], ['POST /api/chat'], 'smoke: plain chat produces real streamed assistant text'))
M.append((['History contains the user and assistant conversation'], [f'GET {S}/messages'], 'smoke: history contains the user and assistant conversation'))
M.append((['Real tool roundtrip executes sandbox read'], ['POST /api/chat'], 'smoke: real tool roundtrip executes sandbox read and returns result to model'))
M.append((['Native write archives real workspace output'], [f'POST {S}/input', f'GET {S}/live'], 'smoke: native write archives real workspace output and restores message attachments'))
M.append((['Session switching preserves prior history'], ['GET /api/sessions'], 'smoke: session switching preserves prior history and continued chat'))
M.append((['Native sessions run concurrently and enforce mutation'], ['GET /api/sessions'], 'smoke: native sessions run concurrently and enforce mutation/configuration isolation'))
M.append((['Native plan HTTP, real Pi tool'], [f'GET {S}/plan', f'PUT {S}/plan', f'DELETE {S}/plan'], 'smoke: native plan HTTP, real Pi tool, child parent-plan reading and usage accounting'))
M.append((['Goal continuation executes real Pi tools'], [f'GET {S}/goal', f'PATCH {S}/goal'], 'smoke: goal continuation executes real Pi tools, preserves first-round images and hides internal prompts'))
M.append((['Goal budget stops continuation'], [f'GET {S}/goal'], 'smoke: goal budget stops continuation and runs exactly one real summary round'))
M.append((['Team mode boots the native coordinator'], [f'GET {S}/team'], 'smoke: team mode boots the native coordinator and exposes its real HTTP projection'))
M.append((['Stopping a held Goal closes its model stream'], [f'POST {S}/abort'], 'smoke: stopping a held Goal closes its model stream and resumes the same objective explicitly'))
M.append((['Held child interrupt closes the real stream'], [f'GET {S}/agents'], 'smoke: held child interrupt closes the real stream and preserves follow-up context'))
M.append((['Parent abort cascades to its held children'], [f'GET {S}/agents', f'POST {S}/abort'], 'smoke: parent abort cascades to its held children and preserves another parent child'))
M.append((['Workflow HTTP publishes a DAG'], ['GET /api/workflows', 'POST /api/workflows', 'POST /api/workflows/{workflowId}/run', 'GET /api/workflow-runs/{runId}'], 'smoke: workflow HTTP publishes a DAG and persists two real Pi turns in one public session'))
M.append((['Workflow HTTP approval resolves once'], ['POST /api/workflow-runs/{runId}/approvals/{nodeId}', 'GET /api/workflow-runs/{runId}'], 'smoke: workflow HTTP approval resolves once and cancellation blocks the next Pi prompt'))
M.append((['Stopping an actual held Workflow model stream'], ['POST /api/workflow-runs/{runId}/stop'], 'smoke: stopping an actual held Workflow model stream releases its owner and blocks overlapping mutation'))
M.append((['Workflow unavailable selected model persists failure'], ['GET /api/workflow-runs/{runId}'], 'smoke: workflow unavailable selected model persists failure without a completed node'))
M.append((['Disabled Schedule manual run persists actual Pi output'], ['GET /api/schedules', 'POST /api/schedules', 'POST /api/schedules/{scheduleId}/run'], 'smoke: disabled schedule manual run persists actual Pi output and keeps nextRunAt unchanged'))
M.append((['Workflow media binary roundtrip and fixed native image-engine'], ['POST /api/workflow-media', 'GET /api/workflow-media/{mediaId}/content', 'GET /api/workflow-image-models'], 'smoke: workflow media binary roundtrip and fixed native image-engine HTTP catalog match their contracts'))
M.append((['Notifications use the release config'], ['GET /api/settings/notifications', 'PATCH /api/settings/notifications/browser'], 'smoke: notifications use the release config and preserve other provider files'))
M.append((['Notification template editing and test previews'], ['PUT /api/settings/notifications/templates/{templateId}/{channel}', 'POST /api/settings/notifications/templates/{templateId}/{channel}/test'], 'smoke: notification template editing and test previews do not enqueue browser events'))
M.append((['TUI chat reports return 202'], ['POST /api/settings/notifications/chat-completed', 'POST /api/settings/notifications/chat-waiting'], 'smoke: TUI chat reports return 202 and avoid duplicate browser notifications'))
M.append((['Workflow browser notifications are durable'], ['GET /api/settings/notifications/browser/events'], 'smoke: workflow browser notifications are durable and use release UUID cursor semantics'))
M.append((['New sessions persist complete empty file-change markers'], [f'GET {S}/file-changes'], 'smoke: new sessions persist complete empty file-change markers'))
M.append((['Actual Pi write/edit preserve the first baseline, diff and approval state'], [f'GET {S}/file-changes/diff', f'POST {S}/file-changes/approve'], 'smoke: actual Pi write/edit preserve the first baseline, diff and approval state'))
M.append((['New-file diff uses /dev/null and actual revert removes'], [f'GET {S}/file-changes/diff', f'POST {S}/file-changes/revert'], 'smoke: new-file diff uses /dev/null and actual revert removes the created file'))
M.append((['An active real model stream rejects file-change revert'], [f'POST {S}/file-changes/revert'], 'smoke: an active real model stream rejects file-change revert with HTTP 409'))
M.append((['Actual shell execution marks coverage partial'], [f'GET {S}/change-summary'], 'smoke: actual shell execution marks coverage partial before reporting any zero'))
M.append((['Child tool snapshots belong to the parent'], [f'GET {S}/file-changes'], 'smoke: child tool snapshots belong to the parent and deletion cannot revive them'))
M.append((['Web search catalog and config follow the release Bing contract'], ['GET /api/plugins', 'POST /api/plugins/web-search/test'], 'smoke: web search catalog and config follow the release Bing contract'))
M.append((['Actual Pi web_search emits a progress update'], ['POST /api/plugins/web-search/test'], 'smoke: actual Pi web_search emits a progress update and a native validation error'))
M.append((['Custom UI imports actual ZIP bytes'], ['POST /api/custom-ui/import', 'GET /api/custom-ui/components'], 'smoke: custom UI imports actual ZIP bytes and preserves the original manifest'))
M.append((['Custom UI import conflicts and reserved IDs'], ['POST /api/custom-ui/import'], 'smoke: custom UI import conflicts and reserved IDs leave existing components unchanged'))
M.append((['Custom UI rejects unsafe ZIP paths'], ['POST /api/custom-ui/import'], 'smoke: custom UI rejects unsafe ZIP paths, hidden files, invalid manifests and missing entries'))
M.append((['Custom UI component listing immediately rescans'], ['GET /api/custom-ui/components'], 'smoke: custom UI component listing immediately rescans owned manifest changes'))
M.append((['Custom UI scoped views render without cookies'], ['POST /api/custom-ui/components/{componentId}/views', 'GET /api/custom-ui/bridge.js'], 'smoke: custom UI scoped views render without cookies and retain an opaque CSP and bridge URLs'))
M.append((['Custom UI view credentials cannot authenticate normal APIs'], ['POST /api/custom-ui/components/{componentId}/views'], 'smoke: custom UI view credentials cannot authenticate normal APIs or legacy resources'))
M.append((['Custom UI views reject cross-component files'], ['GET /api/custom-ui/components/{componentId}/assets/{asset1}'], 'smoke: custom UI views reject cross-component files, manifests, hidden paths and traversal'))
M.append((['Custom UI views renew and revoke'], ['PUT /api/custom-ui/views/{viewId}', 'DELETE /api/custom-ui/views/{viewId}'], 'smoke: custom UI views renew and revoke without leaving a usable resource token'))
M.append((['Game visual provider is configured through the native provider API'], ['POST /api/providers/{providerId}/models/discover'], 'smoke: game visual provider is configured through the native provider API'))
M.append((['Game asset catalog and independent draft CRUD obey the release schema'], ['GET /api/game-assets', 'POST /api/game-assets/projects', 'DELETE /api/game-assets/projects/{projectId}', 'PATCH /api/game-assets/projects/{projectId}'], 'smoke: game asset catalog and independent draft CRUD obey the release schema'))
M.append((['Game media raw upload, native color processing'], ['POST /api/game-assets/media', 'GET /api/game-assets/media/{mediaId}/content', 'POST /api/game-assets/process'], 'smoke: game media raw upload, native color processing and workflow media isolation'))
M.append((['Game generation uses native provider HTTP then edits and exports'], ['POST /api/game-assets/projects/{projectId}/run', 'GET /api/game-assets/jobs/{jobId}'], 'smoke: game generation uses native provider HTTP then edits and exports from immutable originals'))
M.append((['Game jobs enforce two active projects and stop'], ['POST /api/game-assets/jobs/{jobId}/stop'], 'smoke: game jobs enforce two active projects and stop their actual image HTTP connections'))
M.append((['Game paid generation failure retains completed direction pixels'], ['GET /api/game-assets/jobs/{jobId}'], 'smoke: game paid generation failure retains completed direction pixels without retry'))
M.append((['Actual Pi image_assets uses native pixels'], [f'POST {S}/input'], 'smoke: actual Pi image_assets uses native pixels and provider HTTP, archives public exports and rejects undelegated calls'))
M.append((['Visual APIs and actual Pi discovery/gateway generate'], ['GET /api/visual/models', 'PUT /api/visual/models/{kind}', 'POST /api/visual/test'], 'smoke: visual APIs and actual Pi discovery/gateway generate, edit and poll video with durable assets'))
M.append((['Native Telegram inbound updates execute real Pi tools'], ['GET /api/channels', 'PATCH /api/channels/{channel}', 'POST /api/channels/{channel}/reconnect'], 'smoke: native telegram inbound updates execute real Pi tools, deliver assets and notifications'))
M.append((['Plugin inspect/install retains exact source bytes'], ['POST /api/plugins/inspect', 'POST /api/plugins/install'], 'smoke: plugin inspect/install retains exact source bytes and rejects inspection tampering'))
M.append((['Actual call_tool runs native fs/path/Buffer'], ['GET /api/plugins'], 'smoke: actual call_tool runs native fs/path/Buffer with the current Pi session context'))
M.append((['Plugin/capability enable state and full-access mode guard'], ['PATCH /api/plugins/{pluginId}', 'PATCH /api/plugins/{pluginId}/capabilities/{capabilityName}'], 'smoke: plugin/capability enable state and full-access mode guard real execution'))
M.append((['Real native plugin exceptions propagate through the Pi gateway'], ['GET /api/plugins'], 'smoke: real native plugin exceptions propagate through the Pi gateway'))
M.append((['Actual plugin_create installs global source'], ['GET /api/plugins', 'POST /api/plugins/install'], 'smoke: actual plugin_create installs global source and exposes its tool on the next turn'))
M.append((['An active Rust plugin worker rejects uninstall and abort'], ['DELETE /api/plugins/{pluginId}'], 'smoke: an active Rust plugin worker rejects uninstall and abort stops its actual file writes'))
M.append((['Memory HTTP creates, searches, updates and deletes real SQLite records'], ['GET /api/memory', 'POST /api/memory/nodes', 'GET /api/memory/candidates', 'POST /api/memory/candidates/{candidateId}/{action}', 'POST /api/memory/candidates/reject-all', 'POST /api/memory/spaces', 'PATCH /api/memory/nodes/{memoryId}', 'DELETE /api/memory/nodes/{memoryId}', 'PATCH /api/memory/spaces/{spaceId}', 'DELETE /api/memory/spaces/{spaceId}'], 'smoke: memory HTTP creates, searches, updates and deletes real SQLite records'))
M.append((['Memory dashboard contract'], ['GET /api/memory', 'GET /api/settings/memory'], 'smoke: memory dashboard contract'))
M.append((['Schedules dashboard contract'], ['GET /api/schedules', 'PATCH /api/schedules/{scheduleId}', 'DELETE /api/schedules/{scheduleId}'], 'smoke: schedules dashboard contract'))
M.append((['Workflows dashboard contract'], ['GET /api/workflows'], 'smoke: workflows dashboard contract'))
M.append((['Assets HTTP uploads, deduplicates, previews and streams a byte range'], ['GET /api/assets', 'POST /api/assets', 'GET /api/assets/{assetId}/content', 'GET /api/assets/{assetId}/download', 'DELETE /api/assets/{assetId}'], 'smoke: assets HTTP uploads, deduplicates, previews and streams a byte range'))
M.append((['Native speech HTTP exposes release catalog, settings and cancellation contracts'], ['GET /api/speech/models', 'GET /api/settings/speech', 'PATCH /api/settings/speech', 'GET /api/speech/terms', 'POST /api/speech/cancel'], 'smoke: native speech HTTP exposes release catalog, settings and cancellation contracts'))
M.append((['Restart persists config, sessions, history and functioning chat'], ['GET /api/health'], 'smoke: restart persists config, sessions, history and functioning chat'))
M.append((['Native channel reconnect preserves peer sessions'], ['POST /api/channels/{channel}/reconnect', 'DELETE /api/channels/{channel}'], 'smoke: native channel reconnect preserves peer sessions and sends through the restarted Agent'))
M.append((['Plugin restart preserves install/enabled/data state'], ['GET /api/plugins'], 'smoke: plugin restart preserves install/enabled/data state, closes old workers and expires inspection tokens'))
M.append((['MCP native configuration and actual tool survive process restart'], ['GET /api/mcp'], 'smoke: MCP native configuration and actual tool survive process restart'))
M.append((['Isolated Rust sidecar bootstrap and health'], ['GET /api/health', 'GET /api/runtime/capabilities', 'GET /api/client-info'], 'smoke: isolated Rust sidecar bootstrap and health'))
M.append((['Runtime capabilities advertise actual support'], ['GET /api/runtime/capabilities'], 'smoke: runtime capabilities advertise actual support'))
M.append((['Browser preference bootstrap contract'], ['GET /api/local/browser-preferences', 'PUT /api/local/browser-preferences'], 'smoke: browser preference bootstrap contract'))
M.append((['A historical journal without its snapshot index stays unavailable'], [f'GET {S}/file-changes'], 'smoke: a historical journal without its snapshot index stays unavailable'))
M.append((['Original BOM/newline bytes and first snapshots are staged for actual restart'], [f'GET {S}/file-changes'], 'smoke: original BOM/newline bytes and first snapshots are staged for actual restart'))
M.append((['Goal, Plan, Agents and Team GET preserve seven original empty journals byte-for-byte'], [f'GET {S}/plan', f'GET {S}/goal', f'GET {S}/agents', f'GET {S}/team'], 'smoke: goal, plan, agents and team GET preserve original empty journals byte-for-byte'))
M.append((['Usage today'], ['GET /api/usage/today'], 'smoke: usage accounting exercised via plan/goal checks'))

ev = json.load(io.open(evidence_path, encoding='utf-8'))
entries = dict(ev['entries'])
added = 0
for fragments, endpoints, text in M:
    if not has(*fragments):
        continue
    for endpoint in endpoints:
        if endpoint not in entries:
            entries[endpoint] = {'evidence': text + f' (smoke run {report_path})'}
            added += 1
        else:
            entries[endpoint]['evidence'] += f'; {text}'

ev['entries'] = dict(sorted(entries.items()))
ev['smoke'] = {
    'runner': 'scripts/smoke-rust-usability.mjs --skip-ui',
    'report': report_path,
    'checks': len(report['checks']),
    'passed': len(passed),
    'failed': [f['name'] for f in report['failures']],
}
io.open(evidence_path, 'w', encoding='utf-8', newline='').write(
    json.dumps(ev, ensure_ascii=False, indent=2) + '\n')
print('added', added, 'new endpoint entries; total', len(ev['entries']))
