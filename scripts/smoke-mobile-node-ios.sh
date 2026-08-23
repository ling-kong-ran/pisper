#!/usr/bin/env bash
set -euo pipefail

MATERIALIZED_ROOT="${1:?materialized Node source root is required}"
FRAMEWORK="${2:?NodeMobile.xcframework path is required}"
RUNTIME_ARCHIVE="${3:?embedded Runtime archive path is required}"
WORK_DIR="${4:-$PWD/release/mobile-node-ios-smoke}"
NODE_OUT="$MATERIALIZED_ROOT/out_ios"
APP_OUT="$WORK_DIR/app"
APP="$APP_OUT/Release-iphonesimulator/testnode.app"
BUNDLE_ID="nodejsmobile.test"
UDID=""

cleanup() {
  if [[ -n "$UDID" ]]; then
    xcrun simctl shutdown "$UDID" >/dev/null 2>&1 || true
    xcrun simctl delete "$UDID" >/dev/null 2>&1 || true
  fi
}
trap cleanup EXIT

rm -rf "$NODE_OUT" "$WORK_DIR"
mkdir -p "$NODE_OUT" "$WORK_DIR"
cp -R "$FRAMEWORK" "$NODE_OUT/NodeMobile.xcframework"
"$MATERIALIZED_ROOT/tools/mobile-test/smoke/build-ios-testnode.sh" "$APP_OUT"
test -d "$APP"

mkdir -p "$APP/pisper"
tar -xzf "$RUNTIME_ARCHIVE" -C "$APP/pisper"
cat > "$APP/pisper-smoke.mjs" <<'SMOKE'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const appRoot = resolve(dirname(fileURLToPath(import.meta.url)), 'pisper')
const writableRoot = resolve(process.env.HOME, 'Library', 'Application Support', 'PisperSmoke')
const token = 'pisper-ios-simulator-smoke'
Object.assign(process.env, {
  PISPER_AGENT_DIR: resolve(writableRoot, 'agent'),
  PISPER_APP_ROOT: appRoot,
  PISPER_DESKTOP_TOKEN: token,
  PISPER_FRONTEND_ROOT: resolve(appRoot, 'dist'),
  PISPER_MOBILE_READY_FILE: resolve(writableRoot, 'ready.json'),
  PISPER_RUNTIME_PROFILE: 'mobile-embedded',
  PISPER_WORKSPACE_DIR: resolve(writableRoot, 'workspace'),
})

const { startEmbeddedRuntime } = await import(
  new URL('./pisper/runtime/mobile-embedded.mjs', import.meta.url)
)
let pisper
try {
  pisper = await startEmbeddedRuntime()
  const bootstrap = await fetch(
    `${pisper.url}/_pisper/desktop/bootstrap?token=${encodeURIComponent(token)}`,
    { redirect: 'manual' },
  )
  const cookie = bootstrap.headers.get('set-cookie')?.split(';', 1)[0]
  if (!cookie) throw new Error('Runtime bootstrap did not return an authentication cookie')
  const response = await fetch(`${pisper.url}/api/health`, { headers: { cookie } })
  const health = await response.json()
  if (!response.ok || health.capabilities?.profile !== 'mobile-embedded') {
    throw new Error(`Unexpected Runtime health response: ${JSON.stringify(health)}`)
  }
  console.log(`PISPER_IOS_RUNTIME_SMOKE_OK ${process.version}`)
} finally {
  await pisper?.close()
}
SMOKE

RUNTIME=$(
  xcrun simctl list runtimes -j | python3 -c "import json,sys; rs=sorted([x for x in json.load(sys.stdin)['runtimes'] if x.get('platform')=='iOS' and x.get('isAvailable')], key=lambda x:[int(n) for n in x['version'].split('.')]); print(rs[-1]['identifier'] if rs else '')"
)
DEVICE_TYPE=$(
  xcrun simctl list devicetypes -j | python3 -c "import json,sys,re; ds=sorted([x for x in json.load(sys.stdin)['devicetypes'] if re.match(r'iPhone \\d', x['name'])], key=lambda x:int(re.match(r'iPhone (\\d+)', x['name']).group(1))); print(ds[-1]['identifier'] if ds else '')"
)
test -n "$RUNTIME"
test -n "$DEVICE_TYPE"
UDID=$(xcrun simctl create pisper-mobile-runtime-smoke "$DEVICE_TYPE" "$RUNTIME")
xcrun simctl bootstatus "$UDID" -b

TOKEN=$(/usr/bin/uuidgen | tr 'A-F' 'a-f' | tr -d '-')
xcrun simctl install "$UDID" "$APP"
INSTALLED_APP=$(xcrun simctl get_app_container "$UDID" "$BUNDLE_ID" app)
DATA_DIR=$(xcrun simctl get_app_container "$UDID" "$BUNDLE_ID" data)
RESULT="$DATA_DIR/Documents/result-$TOKEN.txt"
LOG="$WORK_DIR/simulator.log"
rm -f "$RESULT"

xcrun simctl launch --console --terminate-running-process "$UDID" "$BUNDLE_ID" \
  --run-token "$TOKEN" "$INSTALLED_APP/pisper-smoke.mjs" > "$LOG" 2>&1 &
LAUNCH_PID=$!
VERDICT=""
for ((i = 0; i < 240; i++)); do
  if [[ -s "$RESULT" ]]; then
    VERDICT=$(tr -d '\r\n' < "$RESULT")
    break
  fi
  if ! kill -0 "$LAUNCH_PID" 2>/dev/null; then
    [[ -f "$RESULT" ]] && VERDICT=$(tr -d '\r\n' < "$RESULT")
    break
  fi
  sleep 1
done
kill "$LAUNCH_PID" 2>/dev/null || true
wait "$LAUNCH_PID" 2>/dev/null || true
cat "$LOG"
test "$VERDICT" = PASS
grep -Fq 'PISPER_IOS_RUNTIME_SMOKE_OK' "$LOG"
echo "iOS embedded Pisper Runtime smoke passed"
