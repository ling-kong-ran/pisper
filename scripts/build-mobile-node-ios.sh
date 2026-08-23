#!/usr/bin/env bash
# 从固定 recipe commit 冷构建 iOS NodeMobile.xcframework，并记录源树与每个文件摘要。
set -Eeuo pipefail

ROOT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
METADATA="$ROOT_DIR/scripts/mobile-node-artifacts.json"
WORK_DIR=${PISPER_NODE_MOBILE_IOS_WORK_DIR:-"$ROOT_DIR/release/mobile-node-ios-work"}
OUTPUT_DIR=${PISPER_NODE_MOBILE_IOS_OUTPUT:-"$ROOT_DIR/release/mobile-node-ios"}
SOURCE_REPOSITORY=$(node -p "JSON.parse(require('fs').readFileSync(process.argv[1])).source.repository" "$METADATA")
SOURCE_COMMIT=$(node -p "JSON.parse(require('fs').readFileSync(process.argv[1])).source.commit" "$METADATA")
RECIPE_TREE=$(node -p "JSON.parse(require('fs').readFileSync(process.argv[1])).source.recipeTree" "$METADATA")
MATERIALIZED_TREE=$(node -p "JSON.parse(require('fs').readFileSync(process.argv[1])).source.materializedTree" "$METADATA")
NODE_VERSION=$(node -p "JSON.parse(require('fs').readFileSync(process.argv[1])).runtime.nodeVersion" "$METADATA")

rm -rf "$WORK_DIR" "$OUTPUT_DIR"
mkdir -p "$WORK_DIR" "$OUTPUT_DIR"
git init -q "$WORK_DIR/recipe"
git -C "$WORK_DIR/recipe" remote add origin "$SOURCE_REPOSITORY"
git -C "$WORK_DIR/recipe" fetch --depth=1 origin "$SOURCE_COMMIT"
git -C "$WORK_DIR/recipe" checkout -q --detach FETCH_HEAD
test "$(git -C "$WORK_DIR/recipe" rev-parse HEAD)" = "$SOURCE_COMMIT"
test "$(git -C "$WORK_DIR/recipe" rev-parse 'HEAD^{tree}')" = "$RECIPE_TREE"

python3.12 -m venv "$WORK_DIR/venv"
"$WORK_DIR/venv/bin/pip" install --disable-pip-version-check setuptools
(
  cd "$WORK_DIR/recipe"
  PATH="$WORK_DIR/venv/bin:$PATH" scripts/prepare.sh "$WORK_DIR/materialized"
)
test "$(git -C "$WORK_DIR/materialized" rev-parse 'HEAD^{tree}')" = "$MATERIALIZED_TREE"
(
  cd "$WORK_DIR/materialized"
  PATH="$WORK_DIR/venv/bin:$PATH" ./tools/ios_framework_prepare.sh
)

test -d "$WORK_DIR/materialized/out_ios/NodeMobile.xcframework"
cp -R "$WORK_DIR/materialized/out_ios/NodeMobile.xcframework" "$OUTPUT_DIR/"
cp "$WORK_DIR/materialized/LICENSE" "$OUTPUT_DIR/LICENSE.nodejs"
(
  cd "$OUTPUT_DIR"
  find NodeMobile.xcframework -type f -print0 | LC_ALL=C sort -z | xargs -0 shasum -a 256 > SHA256SUMS
)
node - "$METADATA" "$OUTPUT_DIR/pisper-node-artifact.json" <<'NODE'
const fs = require('node:fs')
const [metadataPath, outputPath] = process.argv.slice(2)
const metadata = JSON.parse(fs.readFileSync(metadataPath, 'utf8'))
fs.writeFileSync(
  outputPath,
  `${JSON.stringify(
    {
      schemaVersion: metadata.schemaVersion,
      source: metadata.source,
      runtime: metadata.runtime,
      platform: 'ios',
      architectures: ['arm64', 'arm64-simulator'],
      xcode: require('node:child_process').execFileSync('xcodebuild', ['-version'], { encoding: 'utf8' }).trim(),
    },
    null,
    2,
  )}\n`,
)
NODE

test "$NODE_VERSION" = "24.18.1"
printf 'iOS embedded Node staged: %s\n' "$OUTPUT_DIR"
