$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

$openSsh = Get-ItemProperty -LiteralPath 'HKLM:\SOFTWARE\OpenSSH' -ErrorAction SilentlyContinue
$defaultShell = if (
    $null -ne $openSsh -and
    -not [string]::IsNullOrWhiteSpace([string]$openSsh.DefaultShell)
) {
    [string]$openSsh.DefaultShell
} else {
    'cmd.exe'
}

function Get-Candidate([string]$Path, [bool]$Sidecar) {
    if ([string]::IsNullOrWhiteSpace($Path) -or -not [IO.File]::Exists($Path)) {
        return $null
    }
    if ($Sidecar) {
        $env:HERDR_REMOTE_SIDECAR_V1 = '1'
        Remove-Item Env:HERDR_ENV -ErrorAction SilentlyContinue
    } else {
        Remove-Item Env:HERDR_REMOTE_SIDECAR_V1 -ErrorAction SilentlyContinue
    }
    $clientLines = @(& $Path 'status' 'client' '--json')
    if ($LASTEXITCODE -ne 0) {
        return $null
    }
    try {
        $client = (($clientLines -join "`n").Trim() | ConvertFrom-Json)
    } catch {
        return $null
    }
    $matchesCurrent = (
        [string]$client.version -ceq $ExpectedRuntime -and
        [uint32]$client.protocol -eq $ExpectedProtocol
    )
    if ($Sidecar) {
        $a = @($V)
        if ($matchesCurrent -and $null -ne $ExpectedPayloadSha256) {
            $a += $ExpectedPayloadSha256
        }
        & $Path @a | Out-Null
        if ($LASTEXITCODE -ne 0 -and $matchesCurrent -and $null -ne $ExpectedPayloadSha256) {
            & $Path $V | Out-Null
            $matchesCurrent = $false
        }
        if ($LASTEXITCODE -ne 0) {
            return $null
        }
    }
    $serverArguments = @($ServerArguments) + @('status', 'server', '--json')
    $serverLines = @(& $Path @serverArguments)
    if ($LASTEXITCODE -ne 0) {
        exit $LASTEXITCODE
    }
    try {
        $server = (($serverLines -join "`n").Trim() | ConvertFrom-Json)
    } catch {
        throw 'matching Herdr binary returned invalid server status JSON'
    }
    return [pscustomobject]@{
        path = $Path
        sidecar = $Sidecar
        matches_current = $matchesCurrent
        client = $client
        server = $server
    }
}

$pathCandidate = $null
if ($AllowPathCandidate) {
    $commands = @(Get-Command -Name 'herdr.exe' -CommandType Application -ErrorAction SilentlyContinue)
    if ($commands.Count -gt 0) {
        $pathCandidate = Get-Candidate ([string]$commands[0].Source) $false
    }
}
$sidecarPath = [IO.Path]::Combine(
    $env:USERPROFILE,
    '.herdr',
    'remote',
    'herdr.exe'
)
$sidecarCandidate = if ($null -eq $pathCandidate -or -not $pathCandidate.matches_current) {
    Get-Candidate $sidecarPath $true
} else {
    $null
}
$candidate = if ($null -ne $pathCandidate -and $pathCandidate.matches_current) {
    $pathCandidate
} elseif ($null -ne $sidecarCandidate -and $sidecarCandidate.matches_current) {
    $sidecarCandidate
} elseif ($null -ne $pathCandidate) {
    $pathCandidate
} else {
    $sidecarCandidate
}
[ordered]@{
    os = 'Windows_NT'
    arch = [string]$env:PROCESSOR_ARCHITECTURE
    user_profile = [string]$env:USERPROFILE
    default_shell = $defaultShell
    candidate = $candidate
} | ConvertTo-Json -Compress -Depth 8
exit 0
