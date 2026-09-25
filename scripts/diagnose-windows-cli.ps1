param(
    [ValidateRange(0, 60)]
    [int]$MonitorSeconds = 30,
    [string]$OutputPath = ''
)

# 只报告固定字段；命令行仅在内存中匹配，禁止输出原始错误、个人路径或参数。
$ErrorActionPreference = 'Stop'
$report = [ordered]@{
    schemaVersion = 1
    windows = ($env:OS -eq 'Windows_NT')
    managedCliExists = $false
    managedCliPeHeader = $false
    managedCliReadable = $false
    directVersionStatus = 'not-run'
    directVersionRecognized = $false
    shellInspectionSucceeded = $false
    shellFindsPisper = $false
    shellFirstIsManagedCli = $false
    shellHasAliasOrFunction = $false
    shellHasOtherExecutable = $false
    whereInspectionSucceeded = $false
    whereFindsManagedCli = $false
    whereFindsOtherExecutable = $false
    monitoringRequested = ($MonitorSeconds -gt 0)
    processSampleSucceeded = $false
    processSampleFailed = $false
    processCommandLineUnavailable = $false
    managedCliMentionedByProcess = $false
    possibleInvokerExecutable = $null
    possibleCallerExecutable = $null
}

function Write-Report {
    $json = $report | ConvertTo-Json
    Write-Output $json
    if ($OutputPath) {
        try { [IO.File]::WriteAllText($OutputPath, $json, [Text.UTF8Encoding]::new($true)) }
        catch {
            Write-Host 'Could not save the report. Copy the JSON above instead.'
            exit 1
        }
    }
}

if (-not $report.windows) {
    Write-Report
    return
}

$managedPath = Join-Path ([Environment]::GetFolderPath('LocalApplicationData')) 'com.lingkongran.pisper\cli\pisper.exe'
try {
    $report.managedCliExists = Test-Path -LiteralPath $managedPath -PathType Leaf
    if ($report.managedCliExists) {
        $reader = $null
        try {
            $reader = [IO.BinaryReader]::new([IO.File]::OpenRead($managedPath))
            if ($reader.BaseStream.Length -ge 64 -and $reader.ReadUInt16() -eq 0x5A4D) {
                $reader.BaseStream.Position = 0x3C
                $peOffset = $reader.ReadUInt32()
                if ($peOffset -ge 64 -and $peOffset -le ($reader.BaseStream.Length - 4)) {
                    $reader.BaseStream.Position = $peOffset
                    $report.managedCliPeHeader = ($reader.ReadUInt32() -eq 0x00004550)
                }
            }
            $report.managedCliReadable = $true
        } finally { if ($null -ne $reader) { $reader.Dispose() } }
    }
} catch { $report.managedCliReadable = $false }

if ($report.managedCliPeHeader) {
    $probe = [Diagnostics.Process]::new()
    $probeStarted = $false
    try {
        $probe.StartInfo.FileName = $managedPath
        $probe.StartInfo.Arguments = '--version'
        $probe.StartInfo.UseShellExecute = $false
        $probe.StartInfo.CreateNoWindow = $true
        $probe.StartInfo.RedirectStandardOutput = $true
        $probe.StartInfo.RedirectStandardError = $true
        $probeStarted = $probe.Start()
        # 版本输出应很短；异常程序大量输出时宁可超时，避免诊断本身无界分配内存。
        $stdoutBuffer = New-Object char[] 512
        $stderrBuffer = New-Object char[] 512
        $stdout = $probe.StandardOutput.ReadBlockAsync($stdoutBuffer, 0, $stdoutBuffer.Length)
        $stderr = $probe.StandardError.ReadBlockAsync($stderrBuffer, 0, $stderrBuffer.Length)
        if ($probe.WaitForExit(3000)) {
            $report.directVersionStatus = if ($probe.ExitCode -eq 0) { 'success' } else { 'nonzero-exit' }
            if ($stdout.Wait(100)) {
                $versionText = [string]::new($stdoutBuffer, 0, $stdout.Result)
                $report.directVersionRecognized = $versionText.Trim() -match '^pisper [0-9]+\.[0-9]+\.[0-9]+(?:[-+][0-9A-Za-z.-]+)?$'
            }
        } else {
            $report.directVersionStatus = 'timeout'
        }
    } catch { $report.directVersionStatus = 'unavailable' }
    finally {
        # 超时或读取异常均回收自己创建的探针，不触碰已运行的用户程序。
        if ($probeStarted) {
            try {
                if (-not $probe.HasExited) {
                    $probe.Kill()
                    $null = $probe.WaitForExit(500)
                }
            } catch { $report.directVersionStatus = 'unavailable' }
        }
        $probe.Dispose()
    }
}

try {
    $commands = @(Get-Command pisper -All -ErrorAction SilentlyContinue)
    $report.shellFindsPisper = $commands.Count -gt 0
    foreach ($command in $commands) {
        if ($command.CommandType -in @('Alias', 'Function', 'Filter')) {
            $report.shellHasAliasOrFunction = $true
        } elseif ($command.CommandType -in @('Application', 'ExternalScript')) {
            if ($command.Path -ne $managedPath) { $report.shellHasOtherExecutable = $true }
        }
    }
    if ($commands.Count -gt 0) {
        $report.shellFirstIsManagedCli = ($commands[0].CommandType -eq 'Application' -and $commands[0].Path -eq $managedPath)
    }
    $report.shellInspectionSucceeded = $true
} catch { $report.shellInspectionSucceeded = $false }

try {
    $wherePath = Join-Path ([Environment]::GetFolderPath('System')) 'where.exe'
    $resolved = @(& $wherePath pisper 2>$null)
    $report.whereInspectionSucceeded = $LASTEXITCODE -in @(0, 1)
    foreach ($resolvedPath in $resolved) {
        if ($resolvedPath -eq $managedPath) { $report.whereFindsManagedCli = $true }
        else { $report.whereFindsOtherExecutable = $true }
    }
} catch { $report.whereInspectionSucceeded = $false }

if ($MonitorSeconds -gt 0) {
    Write-Host ('Monitoring for ' + $MonitorSeconds + ' seconds. Reproduce the failing action now, then copy the JSON report below.')
    $timer = [Diagnostics.Stopwatch]::StartNew()
    while ($timer.Elapsed.TotalSeconds -lt $MonitorSeconds) {
        try {
            # Electron 等程序也能执行 Node，按目标参数筛选而非只看 node.exe。
            $candidates = @(Get-CimInstance Win32_Process -Filter "CommandLine LIKE '%com.lingkongran.pisper%cli%pisper.exe%'" -Property Name, ExecutablePath, CommandLine, ParentProcessId -OperationTimeoutSec 1)
            $report.processSampleSucceeded = $true
            foreach ($candidate in $candidates) {
                if (-not $candidate.CommandLine) {
                    $report.processCommandLineUnavailable = $true
                    continue
                }
                if ($candidate.ExecutablePath -eq $managedPath) { continue }
                if ($candidate.CommandLine.Replace('/', '\').IndexOf($managedPath, [StringComparison]::OrdinalIgnoreCase) -ge 0) {
                    $firstMatch = -not $report.managedCliMentionedByProcess
                    $report.managedCliMentionedByProcess = $true
                    if ($firstMatch -and $candidate.Name -match '^[\p{L}\p{N}._ -]{1,100}\.exe$') {
                        $report.possibleInvokerExecutable = $candidate.Name
                    }
                    if ($firstMatch -and $timer.Elapsed.TotalSeconds -lt $MonitorSeconds) {
                        try {
                            $parentId = [uint32]$candidate.ParentProcessId
                            $parent = Get-CimInstance Win32_Process -Filter "ProcessId = $parentId" -Property Name -OperationTimeoutSec 1
                            # 只保留可执行文件名；父进程可能已退出，不能据此确认调用脚本。
                            if ($parent.Name -match '^[\p{L}\p{N}._ -]{1,100}\.exe$') {
                                $report.possibleCallerExecutable = $parent.Name
                            }
                        } catch { }
                    }
                }
            }
        } catch { $report.processSampleFailed = $true }
        $remainingMilliseconds = [int](($MonitorSeconds - $timer.Elapsed.TotalSeconds) * 1000)
        if ($remainingMilliseconds -gt 0) { Start-Sleep -Milliseconds ([Math]::Min(100, $remainingMilliseconds)) }
    }
}

Write-Report
