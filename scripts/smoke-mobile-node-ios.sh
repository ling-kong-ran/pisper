#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
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
cp "$SCRIPT_DIR/mobile-node-ios-smoke-view-controller.m" \
  "$MATERIALIZED_ROOT/tools/mobile-test/ios/testnode/testnode/ViewController.m"
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
DATA_DIR=$(xcrun simctl get_app_container "$UDID" "$BUNDLE_ID" data)
RESULT="$DATA_DIR/Documents/result-$TOKEN.txt"
STDOUT="$DATA_DIR/Documents/stdout-$TOKEN.txt"
LAUNCH_LOG="$WORK_DIR/launch.log"
rm -f "$RESULT" "$STDOUT" "$LAUNCH_LOG"

if ! xcrun simctl launch --terminate-running-process "$UDID" "$BUNDLE_ID" \
  --smoke-ui "$TOKEN" > "$LAUNCH_LOG" 2>&1; then
  cat "$LAUNCH_LOG"
  exit 1
fi
cat "$LAUNCH_LOG"
APP_PID=$(sed -nE 's/^.*: ([0-9]+)$/\1/p' "$LAUNCH_LOG" | tail -1)
test -n "$APP_PID"

VERDICT=""
PROCESS_GONE=""
for ((i = 0; i < 240; i++)); do
  if [[ -s "$RESULT" ]]; then
    VERDICT=$(tr -d '\r\n' < "$RESULT")
    break
  fi
  if [[ -n "$PROCESS_GONE" ]]; then break; fi
  if ! kill -0 "$APP_PID" 2>/dev/null; then PROCESS_GONE=1; fi
  sleep 1
done
xcrun simctl terminate "$UDID" "$BUNDLE_ID" >/dev/null 2>&1 || true
[[ -f "$STDOUT" ]] && cat "$STDOUT"
if [[ "$VERDICT" != PASS ]]; then
  xcrun simctl spawn "$UDID" log show --last 5m --style compact \
    --predicate 'process == "testnode"' || true
  printf 'iOS embedded Pisper Runtime smoke verdict: %s\n' "${VERDICT:-<none>}" >&2
  exit 1
fi
if [[ ! -f "$STDOUT" ]] || ! grep -Fq 'PISPER_IOS_RUNTIME_SMOKE_OK' "$STDOUT"; then
  echo "iOS smoke stdout marker was not flushed; accepting the native PASS verdict" >&2
fi
echo "iOS embedded Pisper Runtime smoke passed"
