#!/bin/bash
# Live contract pass over the remaining unverified release endpoints.
# Starts nothing: expects pisper-server on $BASE with isolated dirs.
set -u
B="${BASE:-http://127.0.0.1:5199}"
W="$(cygpath -u "$TEMP" 2>/dev/null || echo /tmp)/parity-verify2"
mkdir -p "$W"
PASS=0; FAIL=0
ok()  { PASS=$((PASS+1)); echo "PASS $1"; }
bad() { FAIL=$((FAIL+1)); echo "FAIL $1 :: $2"; }
# ok_if NAME EXPECTED_STATUS METHOD PATH [BODY_JSON] [CT]
ok_if() {
  local name="$1" want="$2" method="$3" path="$4" body="${5:-}" ct="${6:-application/json}"
  local args=(-s --noproxy '*' -m 30 -o "$W/resp.json" -w '%{http_code}' -X "$method" "$B$path")
  [ -n "$body" ] && args+=(-H "Content-Type: $ct" --data-binary "$body")
  local code; code=$(curl "${args[@]}")
  if [ "$code" = "$want" ]; then ok "$name"; else
    bad "$name" "want $want got $code $(head -c 160 "$W/resp.json")"
  fi
}
jsonq() { /c/Users/13063/anaconda3/python.exe -c "import json,io;d=json.load(io.open(r'$(cygpath -w "$W/resp.json")',encoding='utf-8'));print(eval(sys.argv[1]))" "$1" 2>/dev/null; }

echo "== bootstrap =="
ok_if "GET /api/health" 200 GET /api/health
SID=$(curl -s --noproxy '*' -m 30 -X POST -H 'Content-Type: application/json' -d '{"name":"verify2"}' "$B/api/sessions" | /c/Users/13063/anaconda3/python.exe -c "import json,sys;print(json.load(sys.stdin)['id'])")
[ -n "$SID" ] && ok "POST /api/sessions → SID=$SID" || { bad "create session" "no id"; exit 1; }
S="/api/sessions/$SID"

echo "== settings/config =="
ok_if "GET chat-dock-layout" 200 GET /api/settings/chat-dock-layout
ok_if "PUT chat-dock-layout (any object per release)" 200 PUT /api/settings/chat-dock-layout '{"any":true}'
ok_if "PATCH settings/memory" 200 PATCH /api/settings/memory '{"autoApproveConfidence":80}'
ok_if "GET runtime/diagnostics" 200 GET /api/runtime/diagnostics
ok_if "GET usage/today" 200 GET /api/usage/today
ok_if "GET session-labels" 200 GET /api/session-labels

echo "== providers =="
PID=$(curl -s --noproxy '*' "$B/api/providers" | jsonq "d['providers'][0]['id']" 2>/dev/null)
[ -n "$PID" ] || PID="custom-10-23-80-140-chat"
ok_if "POST providers/{id}/clone" 201 POST "/api/providers/$PID/clone" '{"id":"clone-t","name":"clone-t"}'
CLONE=$(jsonq "d['id']")
ok_if "PUT providers/{id}/enabled" 200 PUT "/api/providers/$CLONE/enabled" '{"enabled":false}'
ok_if "PUT providers/{id}/models/options" 200 PUT "/api/providers/$CLONE/models/options" '{}'
ok_if "POST providers/{id}/models" 201 POST "/api/providers/$CLONE/models" '{"id":"m-test","name":"m-test","contextWindow":8192,"maxTokens":1024}'
ok_if "DELETE providers/{id}/models" 200 DELETE "/api/providers/$CLONE/models" '{"id":"m-test"}'
ok_if "POST providers/{id}/models/batch" 200 POST "/api/providers/$CLONE/models/batch" '{"models":[]}'
ok_if "POST providers/{id}/import (external-CLI seam)" 501 POST "/api/providers/$CLONE/import" '{}'
ok_if "DELETE providers/{id}" 200 DELETE "/api/providers/$CLONE"
ok_if "POST providers/import-local" 200 POST /api/providers/import-local '{}'
ok_if "POST providers/models/refresh" 200 POST /api/providers/models/refresh '{}'

echo "== decisions =="
ok_if "GET decisions/status" 200 GET /api/decisions/status
ok_if "PUT decisions/config" 200 PUT /api/decisions/config '{"mode":"ask"}'
ok_if "POST decisions/test" 200 POST /api/decisions/test '{"input":"hello"}'
ok_if "POST decisions/decide" 200 POST /api/decisions/decide '{"input":"hello"}'

echo "== desktop =="
ok_if "GET app-update (workspace now git-init? exe fallback)" 200 GET /api/app-update
ok_if "GET sponsors placement" 200 GET /api/sponsors/test-placement
ok_if "POST desktop-pet/enabled (no pets → err ok)" 400 POST /api/desktop-pet/enabled '{"enabled":true}'
ok_if "POST desktop-pet/opacity" 200 POST /api/desktop-pet/opacity '{"opacity":0.5}'
ok_if "POST desktop-pet/install (live petdex)" 200 POST /api/desktop-pet/install '{"slug":"alder-catalog-foot"}'
ok_if "POST desktop-pet/select" 200 POST /api/desktop-pet/select '{"slug":"alder-catalog-foot"}'
ok_if "GET desktop-pet/sprite" 200 GET "/api/desktop-pet/sprite?slug=alder-catalog-foot"
ok_if "DELETE desktop-pet/{slug}" 200 DELETE /api/desktop-pet/alder-catalog-foot

echo "== skills =="
mkdir -p "$W/skill-fixture/src-skill"
printf -- '---\nname: verify-skill\ndescription: parity verify fixture skill\n---\n\nDo the verify thing.\n' > "$W/skill-fixture/src-skill/SKILL.md"
ok_if "POST skills/install (local dir)" 201 POST "/api/skills/install" "{\"source\":\"$W/skill-fixture/src-skill\"}"
SKID=$(jsonq "d['installed'][0]['id']")
[ -n "$SKID" ] && ok "skills/install returned id=$SKID" || bad "skills install id" "none"
ok_if "PATCH skills/{id}" 200 PATCH "/api/skills/$SKID" '{"enabled":false}'
ok_if "POST skills/reload" 200 POST /api/skills/reload
ok_if "DELETE skills/{id}" 200 DELETE "/api/skills/$SKID"

echo "== directories/workspace =="
ok_if "GET directories" 200 GET /api/directories
ok_if "GET workspace-entries" 200 GET /api/workspace-entries

echo "== extensions =="
ok_if "GET extensions/market" 200 GET /api/extensions/market
ok_if "GET extensions" 200 GET /api/extensions
ok_if "POST extensions/install (accepted record)" 201 POST /api/extensions/install '{"source":"nonexistent"}'
ok_if "DELETE extensions (none → ok)" 200 DELETE /api/extensions '{}'

echo "== remote =="
ok_if "GET remote/status" 200 GET /api/remote/status
ok_if "GET remote/connection-info" 200 GET /api/remote/connection-info
ok_if "PUT remote/enabled" 200 PUT /api/remote/enabled '{"enabled":false}'
ok_if "POST remote/firewall/retry" 200 POST /api/remote/firewall/retry '{}'
ok_if "POST remote/pairing-code" 200 POST /api/remote/pairing-code
PR=$(curl -s --noproxy '*' -X POST -H 'Content-Type: application/json' -d '{"deviceName":"verify2-phone"}' "$B/api/remote/pairing-requests")
RID=$(echo "$PR" | grep -oE '"requestId":"[^"]+"' | cut -d'"' -f4)
SEC=$(echo "$PR" | grep -oE '"requestSecret":"[^"]+"' | cut -d'"' -f4)
ok_if "DELETE remote/pairing-requests/{id}" 204 DELETE "/api/remote/pairing-requests/$RID" "" "application/json" -H "x-pisper-pairing-secret: $SEC"

echo "== mcp-host rotate =="
ok_if "POST mcp-host/rotate-token (disabled → 400)" 400 POST /api/mcp-host/rotate-token '{}'
ok_if "PATCH mcp-host enable" 200 PATCH /api/mcp-host '{"enabled":true}'
ok_if "POST mcp-host/rotate-token (not listening → 503)" 503 POST /api/mcp-host/rotate-token
ok_if "PATCH mcp-host disable" 200 PATCH /api/mcp-host '{"enabled":false}'

echo "== session controls =="
ok_if "PUT session/model" 200 PUT "$S/model" "{\"provider\":\"custom-10-23-80-140-chat\",\"model\":\"qwen3.8-27b\"}"
ok_if "GET session/thinking-level" 200 GET "$S/thinking-level"
ok_if "PUT session/thinking-level" 200 PUT "$S/thinking-level" '{"level":"off"}'
ok_if "PUT session/cwd" 200 PUT "$S/cwd" "{\"cwd\":\"C:/Users/13063/AppData/Local/Temp/pisper-parity-verify/workspace\"}"
ok_if "PUT session/execution-mode" 200 PUT "$S/execution-mode" '{"mode":"ask"}'
ok_if "PUT session/permission" 200 PUT "$S/permission" '{"mode":"ask"}'
ok_if "PUT session/run-mode" 200 PUT "$S/run-mode" '{"mode":"plan"}'
ok_if "PATCH session/organization" 200 PATCH "$S/organization" '{"pinned":true}'
ok_if "PATCH session rename" 200 PATCH "$S" '{"name":"verify2-renamed"}'
ok_if "POST session/compact (empty → 400 per release mapping)" 400 POST "$S/compact" '{}'
ok_if "GET session/workflow-runs" 200 GET "$S/workflow-runs"
LEAF=$(curl -s --noproxy '*' "$B$S/tree" | jsonq "d['leafId']")
echo "leaf=$LEAF"
[ -n "$LEAF" ] && [ "$LEAF" != "None" ] && ok_if "POST session/derive" 201 POST "$S/derive" "{\"boundaryEntryId\":\"$LEAF\",\"name\":\"verify2-branch\"}"
ok_if "GET session/git/changes" 200 GET "$S/git/changes"
ok_if "POST session/vcs/commit (no repo → 400/500 ok shape)" 500 POST "$S/vcs/commit" '{"message":"x"}' || true
ok_if "POST session/vcs/push (no repo → err)" 500 POST "$S/vcs/push" '{}' || true
ok_if "POST session/vcs/revert (no repo → err)" 500 POST "$S/vcs/revert" '{}' || true
ok_if "POST session/git/revert (no repo → err)" 500 POST "$S/git/revert" '{}' || true
ok_if "DELETE session" 200 DELETE "$S"

echo "== workflows extras =="
WF=$(curl -s --noproxy '*' -X POST -H 'Content-Type: application/json' -d '{"name":"verify2-wf","nodes":[],"inputs":[]}' "$B/api/workflows")
WFID=$(echo "$WF" | jsonq "d['id']")
[ -n "$WFID" ] && ok "create workflow id=$WFID" || bad "create workflow" "$WF"
[ -n "$WFID" ] && ok_if "PATCH workflows/{id}" 200 PATCH "/api/workflows/$WFID" '{"name":"verify2-wf2"}'
[ -n "$WFID" ] && ok_if "GET workflows/{id}/bundle" 200 GET "/api/workflows/$WFID/bundle"
[ -n "$WFID" ] && ok_if "POST workflows/{id}/duplicate" 201 POST "/api/workflows/$WFID/duplicate" '{"name":"verify2-wf-dup"}'
[ -n "$WFID" ] && ok_if "DELETE workflows/{id}" 200 DELETE "/api/workflows/$WFID"
ok_if "POST workflows/import-bundle (bad → 400)" 400 POST /api/workflows/import-bundle '{}'
ok_if "POST workflow-image-process (bad → 4xx)" 400 POST /api/workflow-image-process '{}'
ok_if "GET sprite-engines files (404 ok)" 404 GET "/api/sprite-engines/none/files/none"

echo "== plugins PUT =="
ok_if "PUT /api/plugins" 200 PUT /api/plugins '{"plugins":[]}'

echo "== speech =="
ok_if "POST speech/models/download (unknown model → 4xx)" 400 POST /api/speech/models/download '{"model":"nope"}'
ok_if "POST speech/models/cancel (unknown → 4xx shape)" 400 POST /api/speech/models/cancel '{"model":"nope"}'
ok_if "POST speech/session (no model → 400 invalid)" 400 POST /api/speech/session '{}'
ok_if "POST speech/stream/start (no model → 400 missing)" 400 POST /api/speech/stream/start '{}'
ok_if "POST speech/stream/chunk (bad rate → 400)" 400 POST /api/speech/stream/chunk '{}'
ok_if "POST speech/stream/finish (no session → 404)" 404 POST /api/speech/stream/finish '{}'
ok_if "POST speech/stream/cancel" 200 POST /api/speech/stream/cancel '{}'
ok_if "POST speech/transcribe (empty → 4xx)" 400 POST /api/speech/transcribe '{}'
ok_if "POST speech/synthesize (empty → 4xx)" 400 POST /api/speech/synthesize '{}'
ok_if "POST speech/cancel (empty → 400 invalid)" 400 POST /api/speech/cancel '{}'

echo
echo "TOTAL PASS=$PASS FAIL=$FAIL"
