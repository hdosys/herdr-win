[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string] $ExePath,

    [string] $Session = "conpty-input-$([guid]::NewGuid().ToString('N'))",

    [string] $SocketPath = "",

    [string] $ExpectedConsoleHostPath = "",

    [string] $TerminalPath = "",

    [ValidateRange(30, 300)]
    [int] $TimeoutSeconds = 120
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

function Get-RemainingMilliseconds {
    param([int] $Maximum = 30000)

    $remaining = [int][Math]::Floor(($script:Deadline - [DateTime]::UtcNow).TotalMilliseconds)
    if ($remaining -le 0) {
        throw "enhanced input probe exceeded its bounded deadline"
    }
    return [Math]::Min($remaining, $Maximum)
}

function Invoke-ProcessResult {
    param(
        [string] $Command,
        [string[]] $Arguments,
        [int] $TimeoutMilliseconds = 0
    )

    if ($TimeoutMilliseconds -le 0) {
        $TimeoutMilliseconds = Get-RemainingMilliseconds
    }
    $startInfo = [System.Diagnostics.ProcessStartInfo]::new()
    $startInfo.FileName = $Command
    $startInfo.UseShellExecute = $false
    $startInfo.CreateNoWindow = $true
    $startInfo.RedirectStandardInput = $true
    $startInfo.RedirectStandardOutput = $true
    $startInfo.RedirectStandardError = $true
    foreach ($argument in $Arguments) {
        $startInfo.ArgumentList.Add([string]$argument)
    }

    $process = [System.Diagnostics.Process]::new()
    $process.StartInfo = $startInfo
    if (-not $process.Start()) {
        throw "could not start $Command"
    }
    $process.StandardInput.Close()
    $stdout = $process.StandardOutput.ReadToEndAsync()
    $stderr = $process.StandardError.ReadToEndAsync()
    if (-not $process.WaitForExit($TimeoutMilliseconds)) {
        try { $process.Kill($true) } catch {}
        $process.WaitForExit(5000) | Out-Null
        throw "$Command did not exit within $TimeoutMilliseconds milliseconds"
    }
    $result = [ordered]@{
        exit_code = $process.ExitCode
        stdout = $stdout.GetAwaiter().GetResult()
        stderr = $stderr.GetAwaiter().GetResult()
    }
    $process.Dispose()
    return $result
}

function Invoke-Checked {
    param([string] $Command, [string[]] $Arguments)

    $result = Invoke-ProcessResult -Command $Command -Arguments $Arguments
    if ($result.exit_code -ne 0) {
        $detail = if (-not [string]::IsNullOrWhiteSpace($result.stderr)) {
            $result.stderr.Trim()
        } elseif (-not [string]::IsNullOrWhiteSpace($result.stdout)) {
            $result.stdout.Trim()
        } else {
            "no diagnostic"
        }
        throw "command failed with exit code $($result.exit_code): $Command $($Arguments -join ' '): $detail"
    }
    return $result
}

function Invoke-HerdrJson {
    param([string[]] $Arguments)

    $result = Invoke-Checked -Command $script:Exe -Arguments $Arguments
    try {
        return $result.stdout | ConvertFrom-Json -Depth 40
    } catch {
        throw "Herdr returned invalid JSON for '$($Arguments -join ' ')': $($result.stdout)"
    }
}

function Read-ReportLines {
    param([string] $Path)

    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        return @()
    }
    try {
        $stream = [System.IO.FileStream]::new(
            $Path,
            [System.IO.FileMode]::Open,
            [System.IO.FileAccess]::Read,
            [System.IO.FileShare]::ReadWrite -bor [System.IO.FileShare]::Delete
        )
        try {
            $reader = [System.IO.StreamReader]::new(
                $stream,
                [System.Text.UTF8Encoding]::new($false),
                $true,
                1024,
                $false
            )
            try {
                $text = $reader.ReadToEnd()
            } finally {
                $reader.Dispose()
            }
        } finally {
            $stream.Dispose()
        }
        return @($text -split "\r?\n" | Where-Object { $_.Length -gt 0 })
    } catch [System.IO.IOException] {
        return @()
    }
}

function New-ReportWatcher {
    param([string] $Path)

    $watcher = [System.IO.FileSystemWatcher]::new(
        [System.IO.Path]::GetDirectoryName($Path),
        [System.IO.Path]::GetFileName($Path)
    )
    $watcher.NotifyFilter = [System.IO.NotifyFilters]::FileName -bor `
        [System.IO.NotifyFilters]::LastWrite -bor [System.IO.NotifyFilters]::Size
    $watcher.EnableRaisingEvents = $true
    return $watcher
}

function Wait-ReportReady {
    param([string] $Path, [string] $Mode)

    $needle = "READY:$($Mode.ToUpperInvariant())"
    $watcher = New-ReportWatcher -Path $Path
    try {
        while ($true) {
            $lines = @(Read-ReportLines -Path $Path)
            $probeError = @($lines | Where-Object { $_.StartsWith("ERROR:") })
            if ($probeError.Count -gt 0) {
                throw "$Mode probe failed before readiness: $($probeError -join ', ')"
            }
            if ($lines -contains $needle) {
                return
            }
            $wait = [Math]::Min((Get-RemainingMilliseconds -Maximum 10000), 500)
            $null = $watcher.WaitForChanged(
                [System.IO.WatcherChangeTypes]::Created -bor [System.IO.WatcherChangeTypes]::Changed,
                $wait
            )
        }
    } finally {
        $watcher.Dispose()
    }
}

function Get-LatestProbeHex {
    param([string] $Path)

    $lines = @(Read-ReportLines -Path $Path | Where-Object { $_.StartsWith("HEX:") })
    if ($lines.Count -eq 0) {
        return ""
    }
    return $lines[$lines.Count - 1].Substring(4)
}

function Wait-ReportHex {
    param(
        [string] $Path,
        [scriptblock] $Accept
    )

    $watcher = New-ReportWatcher -Path $Path
    try {
        while ($true) {
            $hex = Get-LatestProbeHex -Path $Path
            if (-not [string]::IsNullOrEmpty($hex) -and (& $Accept $hex)) {
                return $hex
            }
            $wait = [Math]::Min((Get-RemainingMilliseconds -Maximum 10000), 500)
            $null = $watcher.WaitForChanged(
                [System.IO.WatcherChangeTypes]::Created -bor [System.IO.WatcherChangeTypes]::Changed,
                $wait
            )
        }
    } finally {
        $watcher.Dispose()
    }
}

function Wait-ReportHexAppend {
    param(
        [string] $Path,
        [string] $PreviousHex,
        [string] $ExpectedHex
    )

    $observed = Wait-ReportHex -Path $Path -Accept {
        param($hex)
        $hex.StartsWith($PreviousHex) -and `
            $hex.Substring($PreviousHex.Length) -ceq $ExpectedHex
    }
    return [ordered]@{ delivered = $true; observed_hex = $observed }
}

function Wait-NativeRecord {
    param([string] $Path, [string] $ExpectedRecord, [int] $PreviousCount)

    $needle = "RECORD:$ExpectedRecord"
    $watcher = New-ReportWatcher -Path $Path
    try {
        while ($true) {
            $count = @(Read-ReportLines -Path $Path | Where-Object { $_ -ceq $needle }).Count
            if ($count -gt $PreviousCount) {
                return [ordered]@{ delivered = $true; expected_record = $needle }
            }
            $wait = [Math]::Min((Get-RemainingMilliseconds -Maximum 10000), 500)
            $null = $watcher.WaitForChanged(
                [System.IO.WatcherChangeTypes]::Created -bor [System.IO.WatcherChangeTypes]::Changed,
                $wait
            )
        }
    } finally {
        $watcher.Dispose()
    }
}

function Quote-PowerShellLiteral {
    param([string] $Value)
    return "'$($Value.Replace("'", "''"))'"
}

function Wait-PaneExists {
    param([string] $PaneId)

    while ($true) {
        $listed = Invoke-HerdrJson @("pane", "list")
        if (@($listed.result.panes | Where-Object { $_.pane_id -ceq $PaneId }).Count -eq 1) {
            return
        }
        $wait = [Math]::Min((Get-RemainingMilliseconds -Maximum 10000), 100)
        Start-Sleep -Milliseconds $wait
    }
}

function Wait-PaneRuntime {
    param([string] $PaneId)

    while ($true) {
        try {
            $null = Invoke-Checked -Command $script:Exe -Arguments @(
                "pane", "read", $PaneId, "--lines", "1"
            )
            return
        } catch {
            if (-not $_.Exception.Message.Contains('"code":"pane_not_found"')) {
                throw
            }
        }
        $wait = [Math]::Min((Get-RemainingMilliseconds -Maximum 10000), 100)
        Start-Sleep -Milliseconds $wait
    }
}

function Start-ProbeInPane {
    param([string] $Mode, [string] $PaneId)

    $script:Phase = "$Mode`: launching probe"
    $reportPath = Join-Path $script:WorkDir "$Mode.report"
    [System.IO.File]::WriteAllText($reportPath, "", [System.Text.UTF8Encoding]::new($false))
    $command = "& $(Quote-PowerShellLiteral $script:ProbeExe) " +
        "$(Quote-PowerShellLiteral $Mode) $(Quote-PowerShellLiteral $reportPath)"
    $null = Invoke-Checked -Command $script:Exe -Arguments @("pane", "run", $PaneId, $command)
    $script:Phase = "$Mode`: waiting for probe readiness"
    Wait-ReportReady -Path $reportPath -Mode $Mode
    return [ordered]@{ pane_id = $PaneId; report_path = $reportPath }
}

function Wait-TerminalClientProcess {
    param(
        [int] $ServerProcessId,
        [string] $ExitPath,
        [string] $StderrPath
    )

    while ($true) {
        $candidates = @(
            Get-Process -Name herdr -ErrorAction SilentlyContinue |
                Where-Object {
                    $_.Id -ne $ServerProcessId -and
                        $script:InitialHerdrIds -notcontains $_.Id
                } |
                Where-Object {
                    $path = ""
                    try { $path = $_.Path } catch {}
                    $path -ieq $script:Exe
                }
        )
        if ($candidates.Count -gt 1) {
            throw "isolated Windows Terminal launched more than one Herdr client"
        }
        if ($candidates.Count -eq 1) {
            return $candidates[0].Id
        }
        if (Test-Path -LiteralPath $ExitPath -PathType Leaf) {
            $exitCode = [System.IO.File]::ReadAllText($ExitPath).Trim()
            $detail = if (Test-Path -LiteralPath $StderrPath -PathType Leaf) {
                [System.IO.File]::ReadAllText($StderrPath).Trim()
            } else {
                "no diagnostic"
            }
            throw "Windows Terminal client exited before readiness (exit $exitCode): $detail"
        }
        $wait = [Math]::Min((Get-RemainingMilliseconds -Maximum 10000), 100)
        Start-Sleep -Milliseconds $wait
    }
}

function New-ProbePane {
    param([string] $Mode)

    $script:Phase = "$Mode`: creating workspace"
    $created = Invoke-HerdrJson @("workspace", "create", "--cwd", $PWD.Path)
    $paneId = [string]$created.result.root_pane.pane_id
    $workspaceId = [string]$created.result.root_pane.workspace_id
    if ([string]::IsNullOrWhiteSpace($paneId) -or [string]::IsNullOrWhiteSpace($workspaceId)) {
        throw "workspace create did not return exact pane and workspace ownership"
    }
    $script:WorkspaceIds.Add($workspaceId)
    $script:Phase = "$Mode`: waiting for pane identity"
    Wait-PaneExists -PaneId $paneId
    $script:Phase = "$Mode`: waiting for pane runtime"
    Wait-PaneRuntime -PaneId $paneId
    return Start-ProbeInPane -Mode $Mode -PaneId $paneId
}

function Send-KeyAndObserve {
    param([object] $Pane, [string] $Key, [string] $ExpectedHex)

    $script:Phase = "sending key $Key to $($Pane.pane_id)"
    $before = Get-LatestProbeHex -Path $Pane.report_path
    $null = Invoke-Checked -Command $script:Exe -Arguments @(
        "pane", "send-keys", $Pane.pane_id, $Key
    )
    $observed = Wait-ReportHexAppend -Path $Pane.report_path `
        -PreviousHex $before -ExpectedHex $ExpectedHex
    return [ordered]@{
        key = $Key
        expected_hex = $ExpectedHex
        delivered = $observed.delivered
    }
}

function Send-RawAndObserve {
    param([object] $Pane, [string] $Text, [string] $ExpectedHex)

    $script:Phase = "sending raw input to $($Pane.pane_id)"
    $before = Get-LatestProbeHex -Path $Pane.report_path
    $null = Invoke-Checked -Command $script:Exe -Arguments @(
        "pane", "send-text", $Pane.pane_id, $Text
    )
    $observed = Wait-ReportHexAppend -Path $Pane.report_path `
        -PreviousHex $before -ExpectedHex $ExpectedHex
    return [ordered]@{ expected_hex = $ExpectedHex; delivered = $observed.delivered }
}

function Send-NativeRecordAndObserve {
    param([object] $Pane, [string] $Text, [string] $ExpectedRecord)

    $script:Phase = "sending native record to $($Pane.pane_id)"
    $needle = "RECORD:$ExpectedRecord"
    $before = @(Read-ReportLines -Path $Pane.report_path | Where-Object { $_ -ceq $needle }).Count
    $null = Invoke-Checked -Command $script:Exe -Arguments @(
        "pane", "send-text", $Pane.pane_id, $Text
    )
    return Wait-NativeRecord -Path $Pane.report_path `
        -ExpectedRecord $ExpectedRecord -PreviousCount $before
}

function Wait-ServerState {
    param([bool] $Running)

    $last = $null
    while ($true) {
        try {
            $last = Invoke-HerdrJson @("status", "server", "--json")
            if ([bool]$last.running -eq $Running) {
                if (-not $Running) {
                    return $last
                }
                if (-not [string]::Equals(
                    [string]$last.socket,
                    $script:SocketPath,
                    [System.StringComparison]::OrdinalIgnoreCase
                )) {
                    throw "server status reported another socket: $($last.socket)"
                }
                if ([string]$last.session -cne $Session) {
                    throw "server status reported another session: $($last.session)"
                }
                return $last
            }
        } catch {
            $last = $_.Exception.Message
        }
        $wait = [Math]::Min((Get-RemainingMilliseconds -Maximum 10000), 250)
        Start-Sleep -Milliseconds $wait
    }
}

function Wait-ServerClientReady {
    param([string] $Path)

    $prefix = "client socket: "
    $watcher = New-ReportWatcher -Path $Path
    try {
        while ($true) {
            $ready = @(
                Read-ReportLines -Path $Path |
                    Where-Object { $_.StartsWith($prefix, [System.StringComparison]::Ordinal) }
            )
            if ($ready.Count -gt 0) {
                $clientSocket = $ready[$ready.Count - 1].Substring($prefix.Length).Trim()
                if ([string]::IsNullOrWhiteSpace($clientSocket)) {
                    throw "server reported an empty client socket path"
                }
                return $clientSocket
            }
            $wait = [Math]::Min((Get-RemainingMilliseconds -Maximum 10000), 500)
            $null = $watcher.WaitForChanged(
                [System.IO.WatcherChangeTypes]::Created -bor [System.IO.WatcherChangeTypes]::Changed,
                $wait
            )
        }
    } finally {
        $watcher.Dispose()
    }
}

$script:Exe = (Resolve-Path -LiteralPath $ExePath).Path
$script:WorkDir = Join-Path ([System.IO.Path]::GetTempPath()) `
    "herdr-conpty-input-$([guid]::NewGuid().ToString('N'))"
$script:ProbeExe = Join-Path $script:WorkDir "probe.exe"
$probeSource = Join-Path $PSScriptRoot "windows_conpty_input_probe.rs"
$script:WorkspaceIds = [System.Collections.Generic.List[string]]::new()
$script:Deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
$script:Phase = "initialization"
$server = $null
$serverStderr = $null
$terminalClientPid = $null
$terminalClientExitPath = $null
$terminalClientStderrPath = $null
$terminalConsoleHostIds = @()
$report = [ordered]@{}
$failed = $false
$primaryFailure = ""
$cleanupErrors = [System.Collections.Generic.List[string]]::new()
$environmentNames = @(
    "HERDR_SESSION",
    "HERDR_SOCKET_PATH",
    "HERDR_CLIENT_SOCKET_PATH",
    "HERDR_CONFIG_PATH",
    "XDG_CONFIG_HOME",
    "XDG_STATE_HOME",
    "HERDR_LOG"
)
$oldEnvironment = @{}
foreach ($name in $environmentNames) {
    $oldEnvironment[$name] = [System.Environment]::GetEnvironmentVariable(
        $name,
        [System.EnvironmentVariableTarget]::Process
    )
}
$initialConsoleHostIds = @(
    Get-Process -Name conhost, OpenConsole -ErrorAction SilentlyContinue |
        ForEach-Object { $_.Id }
)
$script:InitialHerdrIds = @(
    Get-Process -Name herdr -ErrorAction SilentlyContinue |
        ForEach-Object { $_.Id }
)
$expectedConsoleHost = if ([string]::IsNullOrWhiteSpace($ExpectedConsoleHostPath)) {
    $null
} else {
    (Resolve-Path -LiteralPath $ExpectedConsoleHostPath).Path
}

try {
    New-Item -ItemType Directory -Path $script:WorkDir | Out-Null
    if ([string]::IsNullOrWhiteSpace($SocketPath)) {
        $script:SocketPath = Join-Path $script:WorkDir "herdr.sock"
    } elseif (-not [System.IO.Path]::IsPathFullyQualified($SocketPath)) {
        throw "-SocketPath must be an absolute exact target"
    } else {
        $script:SocketPath = [System.IO.Path]::GetFullPath($SocketPath)
    }

    $configRoot = Join-Path $script:WorkDir "config"
    $stateRoot = Join-Path $script:WorkDir "state"
    $configPath = Join-Path $script:WorkDir "config.toml"
    New-Item -ItemType Directory -Path $configRoot, $stateRoot | Out-Null
    [System.Environment]::SetEnvironmentVariable("HERDR_SESSION", $Session, "Process")
    [System.Environment]::SetEnvironmentVariable(
        "HERDR_SOCKET_PATH", $script:SocketPath, "Process"
    )
    [System.Environment]::SetEnvironmentVariable("HERDR_CLIENT_SOCKET_PATH", $null, "Process")
    [System.Environment]::SetEnvironmentVariable("HERDR_CONFIG_PATH", $configPath, "Process")
    [System.Environment]::SetEnvironmentVariable("XDG_CONFIG_HOME", $configRoot, "Process")
    [System.Environment]::SetEnvironmentVariable("XDG_STATE_HOME", $stateRoot, "Process")
    [System.Environment]::SetEnvironmentVariable("HERDR_LOG", "herdr=info", "Process")

    $version = Invoke-Checked -Command $script:Exe -Arguments @("--version")
    $defaultConfig = Invoke-Checked -Command $script:Exe -Arguments @("--default-config")
    [System.IO.File]::WriteAllText(
        $configPath,
        $defaultConfig.stdout,
        [System.Text.UTF8Encoding]::new($false)
    )
    $null = Invoke-Checked -Command "rustc" -Arguments @(
        "--edition", "2021", $probeSource, "-o", $script:ProbeExe
    )

    $serverStdout = Join-Path $script:WorkDir "server.stdout.log"
    $serverStderr = Join-Path $script:WorkDir "server.stderr.log"
    $server = Start-Process -FilePath $script:Exe -ArgumentList @("server") `
        -PassThru -WindowStyle Hidden -RedirectStandardOutput $serverStdout `
        -RedirectStandardError $serverStderr
    $serverStatus = Wait-ServerState -Running $true
    $script:Phase = "waiting for client listener readiness"
    $serverClientSocket = Wait-ServerClientReady -Path $serverStderr

    $terminalExe = if ([string]::IsNullOrWhiteSpace($TerminalPath)) {
        (Get-Command wt.exe -ErrorAction Stop).Source
    } else {
        (Resolve-Path -LiteralPath $TerminalPath).Path
    }
    $terminalWindow = "herdr-input-$([guid]::NewGuid().ToString('N'))"
    $terminalClientExitPath = Join-Path $script:WorkDir "terminal-client.exit"
    $terminalClientStderrPath = Join-Path $script:WorkDir "terminal-client.stderr.log"
    $terminalClientWrapper = Join-Path $script:WorkDir "terminal-client.cmd"
    $wrapperLines = @(
        "@echo off",
        'set "HERDR_ENV="',
        'set "HERDR_LOG=herdr::client=trace"',
        "`"$script:Exe`" 2> `"$terminalClientStderrPath`"",
        "> `"$terminalClientExitPath`" echo %ERRORLEVEL%",
        "exit /b 0"
    )
    [System.IO.File]::WriteAllLines(
        $terminalClientWrapper,
        $wrapperLines,
        [System.Text.Encoding]::ASCII
    )
    $null = Start-Process -FilePath $terminalExe -ArgumentList @(
        "-w", $terminalWindow,
        "new-tab", "--title", $terminalWindow, "--suppressApplicationTitle",
        "cmd.exe", "/d", "/c", $terminalClientWrapper
    ) -PassThru
    $terminalClientPid = Wait-TerminalClientProcess -ServerProcessId $server.Id `
        -ExitPath $terminalClientExitPath -StderrPath $terminalClientStderrPath
    $terminalConsoleHosts = @(
        Get-Process -Name OpenConsole -ErrorAction SilentlyContinue |
            Where-Object { $initialConsoleHostIds -notcontains $_.Id } |
            ForEach-Object {
                $path = ""
                try { $path = $_.Path } catch {}
                [ordered]@{
                    id = $_.Id
                    path = $path
                }
            }
    )
    if ($terminalConsoleHosts.Count -ne 1) {
        throw "Windows Terminal must own exactly one new OpenConsole process"
    }
    $terminalConsoleHostIds = @($terminalConsoleHosts | ForEach-Object { $_.id })

    $os = Get-CimInstance Win32_OperatingSystem
    $report.identity = [ordered]@{
        binary = $script:Exe
        version = $version.stdout.Trim()
        session = $Session
        socket = $script:SocketPath
        client_socket = $serverClientSocket
        server_binary = $serverStatus.binary
        protocol = $serverStatus.protocol
        terminal = $terminalExe
        terminal_window = $terminalWindow
        terminal_client_pid = $terminalClientPid
        terminal_console_host = $terminalConsoleHosts[0].path
        terminal_client_trace_filter = "herdr::client=trace"
    }
    $report.isolation = [ordered]@{
        config_path = $configPath
        config_root = $configRoot
        state_root = $stateRoot
    }
    $report.os = [ordered]@{
        caption = $os.Caption
        version = $os.Version
        build = $os.BuildNumber
    }
    $legacyPane = New-ProbePane -Mode "legacy"
    $report.legacy_alt_v = Send-KeyAndObserve -Pane $legacyPane `
        -Key "alt+v" -ExpectedHex "1b76"

    $kittyPane = New-ProbePane -Mode "kitty"
    $script:Phase = "kitty: waiting for protocol responses"
    $kittyInitialHex = Wait-ReportHex -Path $kittyPane.report_path -Accept {
        param($hex)
        $hex -match "1b5b3f(?:3[0-9]|3b)+63" -and $hex.Contains("1b5b3f3775")
    }
    $report.device_attributes_response = $kittyInitialHex -match "1b5b3f(?:3[0-9]|3b)+63"
    $report.kitty_query_response = $kittyInitialHex.Contains("1b5b3f3775")
    $report.kitty_alt_v = Send-KeyAndObserve -Pane $kittyPane -Key "alt+v" -ExpectedHex "1b5b3131383b333a3175"
    $report.kitty_ctrl_u = Send-KeyAndObserve -Pane $kittyPane -Key "ctrl+u" -ExpectedHex "1b5b3131373b353a3175"
    $report.kitty_ctrl_v = Send-KeyAndObserve -Pane $kittyPane -Key "ctrl+v" -ExpectedHex "1b5b3131383b353a3175"
    $report.kitty_shift_enter = Send-KeyAndObserve -Pane $kittyPane -Key "shift+enter" -ExpectedHex "1b5b31333b3275"
    $report.kitty_ctrl_backspace = Send-KeyAndObserve -Pane $kittyPane -Key "ctrl+backspace" -ExpectedHex "1b5b3132373b3575"
    $report.kitty_up = Send-KeyAndObserve -Pane $kittyPane -Key "up" -ExpectedHex "1b5b313b313a3141"
    $report.kitty_escape = Send-KeyAndObserve -Pane $kittyPane -Key "esc" -ExpectedHex "1b5b323775"
    $report.raw_kitty_alt_v = Send-RawAndObserve -Pane $kittyPane -Text ([char]27 + "[118;3:1u") -ExpectedHex "1b5b3131383b333a3175"
    $report.raw_kitty_ctrl_u = Send-RawAndObserve -Pane $kittyPane -Text ([char]27 + "[117;5:1u") -ExpectedHex "1b5b3131373b353a3175"
    $report.raw_kitty_ctrl_v = Send-RawAndObserve -Pane $kittyPane -Text ([char]27 + "[118;5:1u") -ExpectedHex "1b5b3131383b353a3175"
    $report.raw_kitty_shift_enter = Send-RawAndObserve -Pane $kittyPane -Text ([char]27 + "[13;2u") -ExpectedHex "1b5b31333b3275"
    $report.raw_kitty_ctrl_backspace = Send-RawAndObserve -Pane $kittyPane -Text ([char]27 + "[127;5u") -ExpectedHex "1b5b3132373b3575"
    $report.raw_kitty_ctrl_delete = Send-RawAndObserve -Pane $kittyPane -Text ([char]27 + "[57426;5u") -ExpectedHex "1b5b35373432363b3575"
    $report.raw_alt_v = Send-RawAndObserve -Pane $kittyPane -Text ([char]27 + "v") -ExpectedHex "1b76"
    $report.raw_ctrl_u = Send-RawAndObserve -Pane $kittyPane -Text ([string][char]0x15) -ExpectedHex "15"

    $nativePane = New-ProbePane -Mode "native"
    $nativeEscapeDown = [char]27 + "[27;1;27;1;0;3_"
    $nativeEscapeRelease = [char]27 + "[27;1;27;0;0;1_"
    $report.native_escape_down = Send-NativeRecordAndObserve -Pane $nativePane `
        -Text $nativeEscapeDown -ExpectedRecord "1;3;27;1;27;0"
    $report.native_escape_release = Send-NativeRecordAndObserve -Pane $nativePane `
        -Text $nativeEscapeRelease -ExpectedRecord "0;1;27;1;27;0"

    $consoleHosts = @(
        Get-Process -Name conhost, OpenConsole -ErrorAction SilentlyContinue |
            Where-Object { $initialConsoleHostIds -notcontains $_.Id } |
            ForEach-Object {
                $path = ""
                $fileVersion = ""
                try { $path = $_.Path } catch {}
                try { $fileVersion = $_.MainModule.FileVersionInfo.FileVersion } catch {}
                [ordered]@{
                    id = $_.Id
                    name = $_.ProcessName
                    path = $path
                    version = $fileVersion
                }
            }
    )
    $report.new_console_hosts = $consoleHosts
    if ($null -ne $expectedConsoleHost) {
        $report.expected_console_host = $expectedConsoleHost
        $report.app_local_console_host = @(
            $consoleHosts | Where-Object {
                $_.name -ieq "OpenConsole" -and $_.path -ieq $expectedConsoleHost
            }
        ).Count -gt 0
    } else {
        $report.app_local_console_host = $null
    }

    $failed = @(
        $report.legacy_alt_v.delivered,
        $report.device_attributes_response,
        $report.kitty_query_response,
        $report.kitty_alt_v.delivered,
        $report.kitty_ctrl_u.delivered,
        $report.kitty_ctrl_v.delivered,
        $report.kitty_shift_enter.delivered,
        $report.kitty_ctrl_backspace.delivered,
        $report.kitty_up.delivered,
        $report.kitty_escape.delivered,
        $report.raw_kitty_alt_v.delivered,
        $report.raw_kitty_ctrl_u.delivered,
        $report.raw_kitty_ctrl_v.delivered,
        $report.raw_kitty_shift_enter.delivered,
        $report.raw_kitty_ctrl_backspace.delivered,
        $report.raw_kitty_ctrl_delete.delivered,
        $report.raw_alt_v.delivered,
        $report.raw_ctrl_u.delivered,
        $report.native_escape_down.delivered,
        $report.native_escape_release.delivered
    ) -contains $false
    if ($null -ne $expectedConsoleHost -and -not $report.app_local_console_host) {
        $failed = $true
    }
} catch {
    $primaryFailure = $_.Exception.Message
    $report.failure_phase = $script:Phase
    $script:Deadline = [DateTime]::UtcNow.AddSeconds(5)
    try {
        $paneDiagnostics = [System.Collections.Generic.List[object]]::new()
        $panes = Invoke-HerdrJson @("pane", "list")
        foreach ($pane in @($panes.result.panes)) {
            try {
                $read = Invoke-Checked -Command $script:Exe -Arguments @(
                    "pane", "read", [string]$pane.pane_id, "--lines", "40"
                )
                $paneDiagnostics.Add([ordered]@{
                    pane_id = $pane.pane_id
                    text = $read.stdout
                })
            } catch {
                $paneDiagnostics.Add([ordered]@{
                    pane_id = $pane.pane_id
                    error = $_.Exception.Message
                })
            }
        }
        $report.pane_diagnostics = @($paneDiagnostics)
        $report.probe_processes = @(
            Get-CimInstance Win32_Process -Filter "Name = 'probe.exe'" |
                Where-Object { $_.ExecutablePath -ieq $script:ProbeExe } |
                ForEach-Object {
                    [ordered]@{
                        process_id = $_.ProcessId
                        parent_process_id = $_.ParentProcessId
                        command_line = $_.CommandLine
                    }
                }
        )
        $report.report_files = @(
            [System.IO.Directory]::GetFiles($script:WorkDir, "*.report") |
                ForEach-Object {
                    [ordered]@{
                        path = $_
                        content = (Read-ReportLines -Path $_) -join "`n"
                    }
                }
        )
    } catch {
        $report.pane_diagnostics_error = $_.Exception.Message
    }
    throw
} finally {
    $script:Deadline = [DateTime]::UtcNow.AddSeconds(20)
    foreach ($workspaceId in $script:WorkspaceIds) {
        try {
            $null = Invoke-Checked -Command $script:Exe -Arguments @(
                "workspace", "close", $workspaceId
            )
        } catch {
            $cleanupErrors.Add("workspace $workspaceId`: $($_.Exception.Message)")
        }
    }

    if ($null -ne $server) {
        try {
            $status = Invoke-HerdrJson @("status", "server", "--json")
            if ([bool]$status.running) {
                $null = Invoke-Checked -Command $script:Exe -Arguments @("server", "stop")
                $null = Wait-ServerState -Running $false
            }
        } catch {
            $cleanupErrors.Add("server stop: $($_.Exception.Message)")
            try {
                $server.Refresh()
                if (-not $server.HasExited) {
                    $server.Kill($true)
                    $server.WaitForExit(5000) | Out-Null
                }
            } catch {
                $cleanupErrors.Add("server process tree: $($_.Exception.Message)")
            }
        }
    }

    if ($null -ne $terminalClientPid) {
        $clientExitDeadline = [DateTime]::UtcNow.AddSeconds(5)
        $clientProcess = $null
        do {
            $clientProcess = Get-Process -Id $terminalClientPid -ErrorAction SilentlyContinue
            if ($null -eq $clientProcess) { break }
            Start-Sleep -Milliseconds 100
        } while ([DateTime]::UtcNow -lt $clientExitDeadline)
        $report.terminal_client_exited = $null -eq $clientProcess
        if (-not $report.terminal_client_exited) {
            try {
                $clientProcess.Kill($true)
                $clientProcess.WaitForExit(5000) | Out-Null
            } catch {}
            $cleanupErrors.Add("Windows Terminal client did not exit after server shutdown")
        }

        $wrapperExitDeadline = [DateTime]::UtcNow.AddSeconds(5)
        while (
            -not (Test-Path -LiteralPath $terminalClientExitPath -PathType Leaf) -and
            [DateTime]::UtcNow -lt $wrapperExitDeadline
        ) {
            Start-Sleep -Milliseconds 100
        }
        if (Test-Path -LiteralPath $terminalClientExitPath -PathType Leaf) {
            $report.terminal_client_exit_code = [int](
                [System.IO.File]::ReadAllText($terminalClientExitPath).Trim()
            )
        } else {
            $cleanupErrors.Add("Windows Terminal client wrapper did not report exit")
        }
    }

    if ($terminalConsoleHostIds.Count -gt 0) {
        $terminalHostExitDeadline = [DateTime]::UtcNow.AddSeconds(5)
        $remainingTerminalHosts = @()
        do {
            $remainingTerminalHosts = @(
                $terminalConsoleHostIds | ForEach-Object {
                    Get-Process -Id $_ -ErrorAction SilentlyContinue
                }
            )
            if ($remainingTerminalHosts.Count -eq 0) { break }
            Start-Sleep -Milliseconds 100
        } while ([DateTime]::UtcNow -lt $terminalHostExitDeadline)
        $report.terminal_console_host_exited = $remainingTerminalHosts.Count -eq 0
        if (-not $report.terminal_console_host_exited) {
            foreach ($hostProcess in $remainingTerminalHosts) {
                try {
                    $hostProcess.Kill($true)
                    $hostProcess.WaitForExit(5000) | Out-Null
                } catch {}
            }
            $cleanupErrors.Add("Windows Terminal OpenConsole process did not exit")
        }
    }

    if (-not [string]::IsNullOrWhiteSpace($primaryFailure)) {
        $report.failure = $primaryFailure
        if (
            $null -ne $terminalClientStderrPath -and
            (Test-Path -LiteralPath $terminalClientStderrPath -PathType Leaf)
        ) {
            $report.terminal_client_stderr = [System.IO.File]::ReadAllText(
                $terminalClientStderrPath
            ).Trim()
        }
        if (
            $null -ne $serverStderr -and
            (Test-Path -LiteralPath $serverStderr -PathType Leaf)
        ) {
            $report.server_stderr = [System.IO.File]::ReadAllText($serverStderr).Trim()
        }
    }

    if ($null -ne $expectedConsoleHost) {
        $hostExitDeadline = [DateTime]::UtcNow.AddSeconds(5)
        $remainingHosts = @()
        do {
            $remainingHosts = @(
                Get-Process -Name OpenConsole -ErrorAction SilentlyContinue |
                    Where-Object { $initialConsoleHostIds -notcontains $_.Id } |
                    Where-Object {
                        $path = ""
                        try { $path = $_.Path } catch {}
                        $path -ieq $expectedConsoleHost
                    }
            )
            if ($remainingHosts.Count -eq 0) { break }
            Start-Sleep -Milliseconds 100
        } while ([DateTime]::UtcNow -lt $hostExitDeadline)
        $report.app_local_console_host_exited = $remainingHosts.Count -eq 0
        if (-not $report.app_local_console_host_exited) {
            $cleanupErrors.Add("app-local OpenConsole process did not exit")
        }
    }

    foreach ($name in $environmentNames) {
        [System.Environment]::SetEnvironmentVariable(
            $name,
            $oldEnvironment[$name],
            [System.EnvironmentVariableTarget]::Process
        )
    }

    $removeDeadline = [DateTime]::UtcNow.AddSeconds(5)
    do {
        Remove-Item -LiteralPath $script:WorkDir -Recurse -Force -ErrorAction SilentlyContinue
        if (-not (Test-Path -LiteralPath $script:WorkDir)) { break }
        Start-Sleep -Milliseconds 100
    } while ([DateTime]::UtcNow -lt $removeDeadline)
    if (Test-Path -LiteralPath $script:WorkDir) {
        $cleanupErrors.Add("temporary probe root remained locked: $script:WorkDir")
    }

    $report.cleanup_errors = @($cleanupErrors)
    $report.cleanup_complete = $cleanupErrors.Count -eq 0
    $report | ConvertTo-Json -Depth 8
}

if ($failed -or $cleanupErrors.Count -ne 0) {
    throw "enhanced Windows ConPTY input probe failed; see the JSON report above"
}
