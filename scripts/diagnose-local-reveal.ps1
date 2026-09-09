param(
    [string]$TargetPath = '',
    [string]$OutputPath = ''
)

# 诊断现有安装，不依赖开发者工具或新版本；不读取会话、密钥或进程命令行。
$ErrorActionPreference = 'Stop'
$report = New-Object 'System.Collections.Generic.List[string]'
function Add-Report([string]$Text) { $report.Add($Text) }

Add-Report 'Pisper local file reveal diagnostics'
Add-Report ('Collected: ' + [DateTimeOffset]::Now.ToString('o'))
Add-Report ('Current user: ' + [Environment]::UserDomainName + '\' + [Environment]::UserName)
Add-Report ('Windows: ' + [Environment]::OSVersion.VersionString)

try {
    $processes = @(Get-CimInstance Win32_Process -Filter "Name = 'Pisper.exe'")
    Add-Report ('Running desktop processes: ' + $processes.Count)
    foreach ($desktopProcess in $processes) {
        Add-Report ('PID: ' + $desktopProcess.ProcessId)
        Add-Report ('Executable: ' + $desktopProcess.ExecutablePath)
        if ($desktopProcess.ExecutablePath) {
            try {
                $version = [Diagnostics.FileVersionInfo]::GetVersionInfo($desktopProcess.ExecutablePath)
                Add-Report ('ProductVersion: ' + $version.ProductVersion + '; FileVersion: ' + $version.FileVersion)
            } catch { Add-Report ('Version read failed: ' + $_.Exception.Message) }
        }
        try {
            $owner = Invoke-CimMethod -InputObject $desktopProcess -MethodName GetOwner
            Add-Report ('Process owner: ' + $owner.Domain + '\' + $owner.User + '; result=' + $owner.ReturnValue)
        } catch { Add-Report ('Process owner unavailable: ' + $_.Exception.Message) }
    }
} catch { Add-Report ('Process inspection failed: ' + $_.Exception.Message) }

$logDirectories = @(
    (Join-Path ([Environment]::GetFolderPath('LocalApplicationData')) 'com.lingkongran.pisper\logs'),
    (Join-Path ([IO.Path]::GetTempPath()) 'com.lingkongran.pisper\logs')
)
foreach ($logDirectory in $logDirectories) {
    Add-Report ''
    Add-Report ('Log directory: ' + $logDirectory)
    try {
        Add-Report ('Directory exists: ' + (Test-Path -LiteralPath $logDirectory -PathType Container))
        $revealLog = Join-Path $logDirectory 'local-reveal.log'
        if (Test-Path -LiteralPath $revealLog -PathType Leaf) {
            $logInfo = Get-Item -LiteralPath $revealLog
            Add-Report ('Reveal log size: ' + $logInfo.Length + '; modified: ' + $logInfo.LastWriteTime.ToString('o'))
            Add-Report 'Last 40 lines:'
            Get-Content -LiteralPath $revealLog -Encoding UTF8 -Tail 40 | ForEach-Object { Add-Report $_ }
        } else { Add-Report 'Reveal log: missing' }
        Add-Report ('Updater log exists: ' + (Test-Path -LiteralPath (Join-Path $logDirectory 'component-updater.log') -PathType Leaf))

        # 不创建应用日志目录，以免掩盖原始的“目录不存在”；仅探测最近已有目录能否写入。
        $probeDirectory = $logDirectory
        while (-not (Test-Path -LiteralPath $probeDirectory -PathType Container)) {
            $probeDirectory = Split-Path -Parent $probeDirectory
            if (-not $probeDirectory) { throw 'No existing parent directory for write probe.' }
        }
        $probePath = Join-Path $probeDirectory ('pisper-reveal-probe-' + [Guid]::NewGuid().ToString('N') + '.tmp')
        $probe = $null
        try {
            $probe = [IO.FileStream]::new(
                $probePath, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write,
                [IO.FileShare]::None, 4096, [IO.FileOptions]::DeleteOnClose
            )
            $probe.WriteByte(0)
            Add-Report ('Write probe succeeded in: ' + $probeDirectory)
        } catch { Add-Report ('Write probe failed in ' + $probeDirectory + ': ' + $_.Exception.Message) }
        finally { if ($null -ne $probe) { $probe.Dispose() } }
    } catch { Add-Report ('Log inspection failed: ' + $_.Exception.Message) }
}

if ($TargetPath) {
    Add-Report ''
    Add-Report ('Requested path: ' + $TargetPath)
    try {
        Add-Report ('Absolute path: ' + [IO.Path]::GetFullPath($TargetPath))
        Add-Report ('File exists: ' + (Test-Path -LiteralPath $TargetPath -PathType Leaf))
        Add-Report ('Directory exists: ' + (Test-Path -LiteralPath $TargetPath -PathType Container))
        $ancestor = $TargetPath
        for ($hop = 1; $hop -le 3; $hop++) {
            $ancestor = Split-Path -Parent $ancestor
            if (-not $ancestor) { break }
            Add-Report ('Parent ' + $hop + ': ' + $ancestor + '; directory exists: ' + (Test-Path -LiteralPath $ancestor -PathType Container))
        }
    } catch { Add-Report ('Path inspection failed: ' + $_.Exception.Message) }
}

Add-Report ''
Add-Report 'Please also provide the original message containing the link and the exact on-screen response after clicking it.'
if (-not $OutputPath) {
    $OutputPath = Join-Path ([IO.Path]::GetTempPath()) ('pisper-local-reveal-' + (Get-Date -Format 'yyyyMMdd-HHmmss') + '-' + [Guid]::NewGuid().ToString('N').Substring(0, 8) + '.txt')
}
[IO.File]::WriteAllLines($OutputPath, $report, [Text.UTF8Encoding]::new($true))
Write-Output ('Report saved to: ' + [IO.Path]::GetFullPath($OutputPath))
