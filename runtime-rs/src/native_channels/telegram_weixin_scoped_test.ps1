$ErrorActionPreference = 'Stop'
$channelTestRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../../..'))
$channelTestDeps = Join-Path $channelTestRoot 'runtime-rs/target/debug/deps'
$channelTestOutput = Join-Path $channelTestRoot 'release/local-rust-build/native-telegram-weixin'
New-Item -ItemType Directory -Path $channelTestOutput -Force | Out-Null
$channelTestBinary = Join-Path $channelTestOutput 'telegram-weixin-tests.exe'
$channelTestArgs = @('--edition=2021', '--test', (Join-Path $PSScriptRoot 'telegram_weixin_harness.rs'), '-A', 'dead_code', '-L', "dependency=$channelTestDeps", '-o', $channelTestBinary)
$channelTestFrozen = @{ axum='54adf3dea2ec31d3'; base64='00384957a60007b4'; chrono='fcdc4f984a0c97ef'; futures='dacc6be2da3229f7'; getrandom='138dd9093d703cf2'; regex='16a1d89aa0fdbc4d'; reqwest='9aa83ca13d224847'; serde_json='e22a2def23e60567'; tokio='10c90d6616eea20d'; tokio_util='e6e734029bfd2d26'; uuid='001c024818f0b5a0' }
foreach ($channelTestName in @('aes','cipher','md5','base64','getrandom','reqwest','serde_json','futures','chrono','uuid','regex','tokio','tokio_util','axum','image','qrcode')) {
    if ($channelTestFrozen.ContainsKey($channelTestName)) {
        $channelTestLibrary = Join-Path $channelTestDeps ('lib' + $channelTestName + '-' + $channelTestFrozen[$channelTestName] + '.rlib')
        if (-not (Test-Path -LiteralPath $channelTestLibrary)) { throw "Missing frozen rlib for $channelTestName" }
        $channelTestArgs += @('--extern', "$channelTestName=$channelTestLibrary")
        continue
    }
    $channelTestCandidates = Get-ChildItem -LiteralPath $channelTestDeps -Filter "lib$channelTestName-*.rlib" | Sort-Object LastWriteTime -Descending
    if ($channelTestName -eq 'getrandom') {
        $channelTestCandidates = $channelTestCandidates | Where-Object {
            $channelTestDepInfo = Join-Path $channelTestDeps ($_.Name -replace '^lib', '' -replace '\.rlib$', '.d')
            (Test-Path -LiteralPath $channelTestDepInfo) -and ((Get-Content -LiteralPath $channelTestDepInfo -Raw) -match 'getrandom-0\.2\.')
        }
    }
    $channelTestLibrary = $channelTestCandidates | Select-Object -First 1
    if (-not $channelTestLibrary) { throw "Missing cached rlib for $channelTestName" }
    $channelTestArgs += @('--extern', "$channelTestName=$($channelTestLibrary.FullName)")
}
& rustc @channelTestArgs
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
& $channelTestBinary --test-threads=1
exit $LASTEXITCODE
