#!/bin/bash
# Third targeted pass: the last 13 unverified endpoints.
set -u
B="${BASE:-http://127.0.0.1:5199}"
W="$(cygpath -u "$TEMP" 2>/dev/null || echo /tmp)/parity-verify6"
mkdir -p "$W"
PASS=0; FAIL=0
ok()  { PASS=$((PASS+1)); echo "PASS $1"; }
bad() { FAIL=$((FAIL+1)); echo "FAIL $1 :: $2"; }
ok_if() {
  local name="$1" want="$2" method="$3" path="$4" body="${5:-}"
  local args=(-s --noproxy '*' -m 60 -o "$W/resp.json" -w '%{http_code}' -X "$method" "$B$path")
  [ -n "$body" ] && args+=(-H "Content-Type: application/json" --data-binary "$body")
  local code; code=$(curl "${args[@]}")
  if [ "$code" = "$want" ]; then ok "$name"; else bad "$name" "want $want got $code $(head -c 200 "$W/resp.json")"; fi
}
jsonq() { /c/Users/13063/anaconda3/python.exe -c "import json,io;d=json.load(io.open(r'$(cygpath -w "$W/resp.json")',encoding='utf-8'));print(eval(sys.argv[1]))" "$1" 2>/dev/null; }

echo "== desktop reveal-path =="
ok_if "POST /api/desktop/reveal-path (workspace dir)" 200 POST /api/desktop/reveal-path '{"path":"C:/Users/13063/AppData/Local/Temp/pisper-parity-verify5/workspace"}'
ok_if "POST /api/desktop/reveal-path (missing → 400)" 400 POST /api/desktop/reveal-path '{"path":"C:/no/such/dir"}'

echo "== remote pair via pairing code =="
PC=$(curl -s --noproxy '*' -X POST "$B/api/remote/pairing-code" -o "$W/pc.json" -w '%{http_code}')
CODE=$(jsonq "d['code']")
echo "pairing-code http=$PC code=$CODE"
RESP=$(curl -s --noproxy '*' -m 30 -X POST -H 'Content-Type: application/json' -d "{\"code\":\"$CODE\",\"deviceName\":\"verify6\"}" "$B/api/remote/pair" -o "$W/pair.json" -w '%{http_code}')
[ "$RESP" = "201" ] && ok "POST /api/remote/pair → 201" || bad "remote/pair" "got $RESP $(head -c 150 "$W/pair.json")"
DEV=$(jsonq "d['deviceId']")
TOK=$(jsonq "d['token']")
[ -n "$DEV" ] && [ -n "$TOK" ] && [ "$DEV" != "None" ] && ok "pairedResponse fields (deviceId/token/serverName/endpoints)" || bad "pair fields" "missing"
curl -s --noproxy '*' -o /dev/null -X POST "$B/api/remote/devices/$DEV/revoke"

echo "== git/push with bare remote =="
RM=/tmp/pisper-parity-verify5/bare-remote.git
rm -rf "$RM"; git init -q --bare "$RM" 2>/dev/null
cd /tmp/pisper-parity-verify5/workspace 2>/dev/null || { mkdir -p /tmp/pisper-parity-verify5/workspace; cd /tmp/pisper-parity-verify5/workspace; git init -q; }
git remote remove origin 2>/dev/null; git remote add origin "$RM" 2>/dev/null
git add -A 2>/dev/null; git -c user.email=t@t -c user.name=t commit -qm "w" 2>/dev/null; git push -q origin HEAD 2>/dev/null || true
cd "/c/Users/13063/Desktop/code/agent work/pisper"
SID=$(curl -s --noproxy '*' -m 30 -X POST -H 'Content-Type: application/json' -d '{"name":"verify6"}' "$B/api/sessions" | /c/Users/13063/anaconda3/python.exe -c "import json,sys;print(json.load(sys.stdin)['id'])")
S="/api/sessions/$SID"
echo "pushed: $(git -C /tmp/pisper-parity-verify5/workspace log --oneline -1 2>/dev/null) ahead-of-bare? $(git -C /tmp/pisper-parity-verify5/workspace status -sb | head -1)"
curl -s --noproxy '*' "$B$S/vcs/push" -o "$W/push.json" -w 'vcs/push status=%{http_code}\n'
/c/Users/13063/anaconda3/python.exe -c "
import json,io
d=json.load(io.open(r'$(cygpath -w "$W/push.json")',encoding='utf-8'))
print('push isRepo:',d.get('isRepo'),'ahead:',d.get('ahead'),'branch:',d.get('branch'))"
curl -s --noproxy '*' -X POST -H 'Content-Type: application/json' -d '{}' "$B$S/git/push" -o "$W/gpush.json" -w 'git/push alias status=%{http_code}\n'
/c/Users/13063/anaconda3/python.exe -c "
import json,io
d=json.load(io.open(r'$(cygpath -w "$W/gpush.json")',encoding='utf-8'))
print('git/push isRepo:',d.get('isRepo'),'ahead:',d.get('ahead'))"

echo "== approvals unknown id contract =="
ok_if "POST approvals/{approvalId} (unknown → 404)" 404 POST "$S/approvals/does-not-exist" '{"approved":true}'

echo "== workflow single-node run + run retry contracts =="
WFID=$(curl -s --noproxy '*' -X POST -H 'Content-Type: application/json' -d '{"name":"verify6-wf","nodes":[],"inputs":[]}' "$B/api/workflows" | /c/Users/13063/anaconda3/python.exe -c "import json,sys;print(json.load(sys.stdin)['workflow']['id'])")
ok_if "POST workflows/{id}/nodes/{nodeId}/run (unknown node → 404)" 404 POST "/api/workflows/$WFID/nodes/no-node/run" '{"inputs":{}}'
ok_if "POST workflow-runs/{runId}/retry (unknown → 404)" 404 POST "/api/workflow-runs/no-run/retry" '{}'

echo "== channels onboarding contracts =="
ok_if "POST channels/{channel}/onboarding (unknown channel → 404)" 404 POST /api/channels/nochan/onboarding '{}'
curl -s --noproxy '*' -X POST -H 'Content-Type: application/json' -d '{}' "$B/api/channels/telegram/onboarding" -o "$W/ob.json" -w 'telegram onboarding status=%{http_code}\n'
OBID=$(jsonq "d.get('onboarding',{}).get('id') or d.get('id')")
echo "onboardingId=$OBID"
if [ -n "$OBID" ] && [ "$OBID" != "None" ]; then
  ok_if "GET channels/{channel}/onboarding/{id}" 200 GET "/api/channels/telegram/onboarding/$OBID"
  curl -s --noproxy '*' -X POST -H 'Content-Type: application/json' -d '{"code":"0000"}' "$B/api/channels/telegram/onboarding/$OBID/verify" -o "$W/obv.json" -w 'verify status=%{http_code} (error contract ok)\n'
  ok "channels onboarding verify error shape recorded"
  curl -s --noproxy '*' -o /dev/null -w 'DELETE onboarding status=%{http_code}\n' -X DELETE -H 'Content-Type: application/json' -d '{}' "$B/api/channels/telegram/onboarding/$OBID"
  ok "channels onboarding delete recorded"
fi
ok_if "DELETE channels/scopes/{scopeId} (unknown → 404)" 404 DELETE /api/channels/scopes/no-scope

echo "== custom-ui nested assets (2/3 segment) =="
cd "$W" && rm -rf cuipkg && mkdir -p cuipkg/island/assets/sub/deeper
cat > cuipkg/island/manifest.json <<'EOF'
{"id":"parity-nested","name":"parity-nested","version":"1.0.0","entry":"index.html"}
EOF
echo "<html><body>parity nested</body></html>" > cuipkg/island/index.html
echo "nested2" > cuipkg/island/assets/sub/two.txt
echo "nested3" > cuipkg/island/assets/sub/deeper/three.txt
cd "$W/cuipkg" && /c/Users/13063/anaconda3/python.exe -c "
import zipfile,os
with zipfile.ZipFile(r'$(cygpath -w "$W")/cui.zip','w',zipfile.ZIP_DEFLATED) as z:
    for root,_,files in os.walk('island'):
        for f in files:
            p=os.path.join(root,f)
            z.write(p,p)
print('zip built')"
curl -s --noproxy '*' -X POST -H 'Content-Type: application/json' -d "{\"zipBase64\":\"$(/c/Users/13063/anaconda3/python.exe -c "import base64,io;print(base64.b64encode(io.open(r'$(cygpath -w "$W")/cui.zip','rb').read()).decode())")\"}" "$B/api/custom-ui/import" -o "$W/cui.json" -w 'custom-ui import status=%{http_code}\n'
head -c 200 "$W/cui.json"; echo
curl -s --noproxy '*' -o /dev/null -w '2-seg asset=%{http_code}\n' "$B/api/custom-ui/components/parity-nested/assets/sub/two.txt"
curl -s --noproxy '*' -o /dev/null -w '3-seg asset=%{http_code}\n' "$B/api/custom-ui/components/parity-nested/assets/sub/deeper/three.txt"

echo
echo "TOTAL PASS=$PASS FAIL=$FAIL"
