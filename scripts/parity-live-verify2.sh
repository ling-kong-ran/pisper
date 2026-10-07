#!/bin/bash
# Targeted second pass: endpoints needing a real provider env or header control.
set -u
B="${BASE:-http://127.0.0.1:5199}"
W="$(cygpath -u "$TEMP" 2>/dev/null || echo /tmp)/parity-verify3"
WINSRC="$(cygpath -m "$W" 2>/dev/null || echo 'C:/x')"
mkdir -p "$W"
PASS=0; FAIL=0
ok()  { PASS=$((PASS+1)); echo "PASS $1"; }
bad() { FAIL=$((FAIL+1)); echo "FAIL $1 :: $2"; }
ok_if() {
  local name="$1" want="$2" method="$3" path="$4" body="${5:-}"
  local args=(-s --noproxy '*' -m 30 -o "$W/resp.json" -w '%{http_code}' -X "$method" "$B$path")
  [ -n "$body" ] && args+=(-H "Content-Type: application/json" --data-binary "$body")
  local code; code=$(curl "${args[@]}")
  if [ "$code" = "$want" ]; then ok "$name"; else bad "$name" "want $want got $code $(head -c 200 "$W/resp.json")"; fi
}
jsonq() { /c/Users/13063/anaconda3/python.exe -c "import json,io;d=json.load(io.open(r'$(cygpath -w "$W/resp.json")',encoding='utf-8'));print(eval(sys.argv[1]))" "$1" 2>/dev/null; }

echo "== boot session with real provider =="
SID=$(curl -s --noproxy '*' -m 30 -X POST -H 'Content-Type: application/json' -d '{"name":"verify3"}' "$B/api/sessions" | /c/Users/13063/anaconda3/python.exe -c "import json,sys;print(json.load(sys.stdin)['id'])")
echo "SID=$SID"; S="/api/sessions/$SID"
ok_if "PUT session/model (real provider)" 200 PUT "$S/model" '{"provider":"custom-10-23-80-140-chat","model":"qwen3.8-27b"}'
ok_if "PUT session/execution-mode (workspace-write)" 200 PUT "$S/execution-mode" '{"mode":"workspace-write"}'
ok_if "PUT session/execution-mode (approval-required)" 200 PUT "$S/execution-mode" '{"mode":"approval-required"}'
ok_if "PUT session/execution-mode (full-access)" 200 PUT "$S/execution-mode" '{"mode":"full-access"}'
ok_if "PUT session/execution-mode (invalid → 400)" 400 PUT "$S/execution-mode" '{"mode":"nope"}'

echo "== chat then derive/compact/retry-adjacent =="
printf '{"sessionId":"%s","message":"Reply with exactly: ok"}' "$SID" > "$W/chat.json"
curl -s --noproxy '*' -m 120 -X POST -H 'Content-Type: application/json' --data-binary @"$W/chat.json" "$B/api/chat" > "$W/chat.out"
grep -qE "^event: done" "$W/chat.out" && ok "chat done for derive setup" || bad "chat done" "no done frame"
curl -s --noproxy '*' "$B$S/tree" -o "$W/tree.json"
LEAF=$(jsonq "d['leafId']")
echo "leaf=$LEAF"
if [ -n "$LEAF" ] && [ "$LEAF" != "None" ]; then
  ok_if "POST session/derive (from assistant leaf)" 201 POST "$S/derive" "{\"boundaryEntryId\":\"$LEAF\",\"name\":\"verify3-branch\"}"
  DERIVED=$(jsonq "d['id']")
  [ -n "$DERIVED" ] && [ "$DERIVED" != "None" ] && ok "derived session id=$DERIVED" || bad "derived id" "none"
fi
ok_if "POST session/compact (below threshold → 400 nothing-to-compact)" 400 POST "$S/compact" '{}'

echo "== skills install (windows-style path, single skill dir) =="
mkdir -p "$W/skills-fixture/verify-skill"
printf -- '---\nname: verify-skill\ndescription: parity verify fixture skill\n---\n\nDo the verify thing.\n' > "$W/skills-fixture/verify-skill/SKILL.md"
rm -rf "$HOME/.pisper/agent/skills/verify-skill"
ok_if "POST skills/install (single skill dir → 201)" 201 POST "/api/skills/install" "{\"source\":\"$WINSRC/skills-fixture/verify-skill\"}"
SKID=$(jsonq "d['installed'][0]['id']")
[ -n "$SKID" ] && [ "$SKID" != "None" ] && ok "skills install id=$SKID" || bad "skills install id" "none"
if [ -n "$SKID" ] && [ "$SKID" != "None" ]; then
  ok_if "PATCH skills/{id} (disable)" 200 PATCH "/api/skills/$SKID" '{"enabled":false}'
  ok_if "DELETE skills/{id}" 200 DELETE "/api/skills/$SKID"
fi

echo "== extensions delete with source =="
ok_if "DELETE /api/extensions (unknown source → 404 per release)" 404 DELETE /api/extensions '{"source":"nonexistent-pkg"}'

echo "== remote pairing delete with secret =="
PR=$(curl -s --noproxy '*' -X POST -H 'Content-Type: application/json' -d '{"deviceName":"verify3-phone"}' "$B/api/remote/pairing-requests")
RID=$(echo "$PR" | grep -oE '"requestId":"[^"]+"' | cut -d'"' -f4)
SEC=$(echo "$PR" | grep -oE '"requestSecret":"[^"]+"' | cut -d'"' -f4)
CODE=$(curl -s --noproxy '*' -o /dev/null -w '%{http_code}' -X DELETE -H "x-pisper-pairing-secret: $SEC" "$B/api/remote/pairing-requests/$RID")
[ "$CODE" = "204" ] && ok "DELETE pairing-requests/{id} with secret → 204" || bad "DELETE pairing request" "got $CODE"

echo "== app-update error contract (workspace HEAD unknown upstream) =="
ok_if "GET app-update (release compare, behind release branch)" 200 GET /api/app-update

echo
echo "TOTAL PASS=$PASS FAIL=$FAIL"
