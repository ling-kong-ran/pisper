# Test-only frozen-dependency harness; does not run Cargo or build the app.
$ErrorActionPreference = 'Stop'
$nativeVisualRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '../../..')).Path
$nativeVisualDeps = (Resolve-Path -LiteralPath (Join-Path $nativeVisualRoot 'runtime-rs/target-web-search-parity/debug/deps')).Path
$nativeVisualOut = [IO.Path]::GetFullPath((Join-Path $nativeVisualRoot 'runtime-rs/target-native-visual-scope'))
if (-not $nativeVisualOut.StartsWith((Join-Path $nativeVisualRoot 'runtime-rs') + [IO.Path]::DirectorySeparatorChar)) { throw 'scope artifacts must stay in runtime-rs target directory' }
New-Item -ItemType Directory -Path $nativeVisualOut -Force | Out-Null
$nativeVisualLibs = @{ axum='54adf3dea2ec31d3'; base64='00384957a60007b4'; chrono='fcdc4f984a0c97ef'; flate2='ea0a4be0baf10298'; futures='dacc6be2da3229f7'; getrandom='138dd9093d703cf2'; icu_collator='38cefeb57f11baf3'; icu_locale_core='a187a3536af388b4'; pi_rust='fab082f639f3937d'; regex='16a1d89aa0fdbc4d'; reqwest='9aa83ca13d224847'; serde_json='e22a2def23e60567'; serde='aed7d71709296e03'; tokio='10c90d6616eea20d'; tokio_util='e6e734029bfd2d26'; uuid='001c024818f0b5a0' }
$nativeVisualArgs = @('--edition=2021','--test',(Join-Path $PSScriptRoot 'harness.rs'),'--crate-name','pisper_native_visual_scope','-L',('dependency='+$nativeVisualDeps),'-o',(Join-Path $nativeVisualOut 'pisper-native-visual-tests.exe'))
foreach ($nativeVisualName in $nativeVisualLibs.Keys) { $nativeVisualArgs += @('--extern',($nativeVisualName+'='+(Join-Path $nativeVisualDeps ('lib'+$nativeVisualName+'-'+$nativeVisualLibs[$nativeVisualName]+'.rlib')))) }
Get-ChildItem -LiteralPath (Join-Path $nativeVisualRoot 'runtime-rs/target-web-search-parity/debug/build') -Directory | ForEach-Object { $nativeVisualNative=Join-Path $_.FullName 'out'; if (Test-Path -LiteralPath $nativeVisualNative) { $nativeVisualArgs += @('-L',('native='+$nativeVisualNative)) } }
$ErrorActionPreference = 'Continue'
& rustc @nativeVisualArgs 2>&1 | Tee-Object -FilePath (Join-Path $nativeVisualOut 'scoped-compile.log')
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
& (Join-Path $nativeVisualOut 'pisper-native-visual-tests.exe') --nocapture 2>&1 | Tee-Object -FilePath (Join-Path $nativeVisualOut 'scoped-tests.log')
exit $LASTEXITCODE
