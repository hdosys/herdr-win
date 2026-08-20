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

function Get-HerdrAttachCandidate([string]$Path, [bool]$Sidecar) {
    if ([string]::IsNullOrWhiteSpace($Path) -or -not [IO.File]::Exists($Path)) {
        return $null
    }
    if ($Sidecar) {
        $env:HERDR_REMOTE_SIDECAR_V1 = '1'
        Remove-Item Env:HERDR_ENV -ErrorAction SilentlyContinue
        $a = @($V)
        if ($null -ne $ExpectedPayloadSha256) {
            $a += $ExpectedPayloadSha256
        }
        & $Path @a | Out-Null
        if ($LASTEXITCODE -ne 0) {
            return $null
        }
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
    if (
        [string]$client.version -cne $ExpectedRuntime -or
        [uint32]$client.protocol -ne $ExpectedProtocol
    ) {
        return $null
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
        client = $client
        server = $server
    }
}

$selected = $null
if ($AllowPathCandidate) {
    $commands = @(Get-Command -Name 'herdr.exe' -CommandType Application -ErrorAction SilentlyContinue)
    if ($commands.Count -gt 0) {
        $selected = Get-HerdrAttachCandidate ([string]$commands[0].Source) $false
    }
}
$sidecarPath = [IO.Path]::Combine(
    $env:USERPROFILE,
    '.herdr',
    'remote',
    'herdr.exe'
)
if ($null -eq $selected) {
    $selected = Get-HerdrAttachCandidate $sidecarPath $true
}
[ordered]@{
    os = 'Windows_NT'
    arch = [string]$env:PROCESSOR_ARCHITECTURE
    user_profile = [string]$env:USERPROFILE
    default_shell = $defaultShell
    selected = $selected
} | ConvertTo-Json -Compress -Depth 8
exit 0
