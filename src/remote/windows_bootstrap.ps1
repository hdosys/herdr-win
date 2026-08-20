Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

function Assert-HerdrRemotePlainDirectory {
    param([Parameter(Mandatory = $true)][string] $Path)

    $item = Get-Item -LiteralPath $Path -Force
    if (-not $item.PSIsContainer -or ($item.Attributes -band [IO.FileAttributes]::ReparsePoint)) {
        throw "unsafe remote sidecar directory: $Path"
    }
}

function Remove-HerdrRemoteSidecar {
    param([Parameter(Mandatory = $true)][string] $Path)

    if (-not [IO.Directory]::Exists($Path)) {
        return
    }
    Assert-HerdrRemotePlainDirectory -Path $Path
    $leasePath = Join-Path $Path '.lease'
    if (-not [IO.File]::Exists($leasePath)) {
        throw "remote sidecar is missing its lease: $Path"
    }

    $lease = [IO.File]::Open(
        $leasePath,
        [IO.FileMode]::Open,
        [IO.FileAccess]::ReadWrite,
        [IO.FileShare]::None
    )
    try {
        foreach ($child in Get-ChildItem -LiteralPath $Path -Force) {
            if ($child.Name -eq '.lease') {
                continue
            }
            if ($child.Attributes -band [IO.FileAttributes]::ReparsePoint) {
                throw "unsafe remote sidecar child: $($child.FullName)"
            }
            if ($child.PSIsContainer) {
                Remove-HerdrRemoteTree -Path $child.FullName
            } else {
                [IO.File]::Delete($child.FullName)
            }
        }
    } finally {
        $lease.Dispose()
    }
    [IO.File]::Delete($leasePath)
    [IO.Directory]::Delete($Path, $false)
}

function Remove-HerdrRemoteTree {
    param([Parameter(Mandatory = $true)][string] $Path)

    if (-not [IO.Directory]::Exists($Path)) {
        return
    }
    Assert-HerdrRemotePlainDirectory -Path $Path
    foreach ($child in Get-ChildItem -LiteralPath $Path -Force) {
        if ($child.Attributes -band [IO.FileAttributes]::ReparsePoint) {
            throw "unsafe remote staging child: $($child.FullName)"
        }
        if ($child.PSIsContainer) {
            Remove-HerdrRemoteTree -Path $child.FullName
        } else {
            [IO.File]::Delete($child.FullName)
        }
    }
    [IO.Directory]::Delete($Path, $false)
}

function Invoke-HerdrRemotePrepareInstall {
    param(
        [Parameter(Mandatory = $true)][string] $Destination,
        [Parameter(Mandatory = $true)][string] $ArchiveName
    )

    $parent = Split-Path -Parent $Destination
    [IO.Directory]::CreateDirectory($parent) | Out-Null
    if ($ArchiveName -cnotmatch '^payload-[0-9]+-[0-9a-f]+\.zip$') {
        throw 'invalid remote payload archive name'
    }
    $archive = Join-Path $parent $ArchiveName
    if ([IO.File]::Exists($archive)) {
        throw "remote payload archive already exists: $archive"
    }
}

function Remove-HerdrRemoteArchive {
    param([Parameter(Mandatory = $true)][string] $Archive)

    if ([IO.File]::Exists($Archive)) {
        [IO.File]::Delete($Archive)
    }
}

function Assert-HerdrRemoteStagePath {
    param(
        [Parameter(Mandatory = $true)][string] $Stage,
        [Parameter(Mandatory = $true)][string] $Destination
    )

    $parent = Split-Path -Parent $Destination
    $stageItem = Get-Item -LiteralPath $Stage -Force
    if (-not $stageItem.PSIsContainer -or
        ($stageItem.Attributes -band [IO.FileAttributes]::ReparsePoint) -or
        $stageItem.Parent.FullName -cne ([IO.DirectoryInfo] $parent).FullName -or
        $stageItem.Name -cnotmatch '^stage-[0-9a-f]{32}$') {
        throw "invalid remote payload stage: $Stage"
    }
}

function Remove-HerdrRemoteStage {
    param(
        [Parameter(Mandatory = $true)][string] $Stage,
        [Parameter(Mandatory = $true)][string] $Destination
    )

    if ([IO.Directory]::Exists($Stage)) {
        Assert-HerdrRemoteStagePath -Stage $Stage -Destination $Destination
        Remove-HerdrRemoteSidecar -Path $Stage
    }
}

function Invoke-HerdrRemoteStageInstall {
    param(
        [Parameter(Mandatory = $true)][string] $Archive,
        [Parameter(Mandatory = $true)][string] $Destination,
        [Parameter(Mandatory = $true)][string] $ExpectedSha256,
        [Parameter(Mandatory = $true)][string] $ExpectedRuntimeVersion,
        [Parameter(Mandatory = $true)][int] $ExpectedProtocol,
        [string] $SessionName = ''
    )

    $parent = Split-Path -Parent $Destination
    $stage = Join-Path $parent ('stage-' + [Guid]::NewGuid().ToString('N'))
    try {
        $actualSha256 = Get-HerdrFileSha256 -Path $Archive
        if ($actualSha256 -ne $ExpectedSha256) {
            throw 'transferred portable payload checksum mismatch'
        }

        Expand-Archive -LiteralPath $Archive -DestinationPath $stage -Force
        Assert-HerdrRemotePlainDirectory -Path $stage
        $exe = Join-Path $stage 'herdr.exe'
        if (-not [IO.File]::Exists($exe)) {
            throw 'portable payload is missing herdr.exe'
        }
        Assert-HerdrPortablePayload -Root $stage

        $status = & $exe status client --json | ConvertFrom-Json
        if ($LASTEXITCODE -ne 0 -or
            [string]$status.version -cne $ExpectedRuntimeVersion -or
            [int]$status.protocol -ne $ExpectedProtocol) {
            throw 'portable payload runtime identity or protocol mismatch'
        }

        $configArguments = @()
        if (-not [string]::IsNullOrEmpty($SessionName)) {
            $configArguments += @('--session', $SessionName)
        }
        $configArguments += @('config', 'check')
        & $exe @configArguments | Out-Null
        if ($LASTEXITCODE -ne 0) {
            throw 'remote Herdr configuration is invalid'
        }

        [IO.File]::WriteAllBytes((Join-Path $stage '.lease'), [byte[]]@())
        [Console]::Out.WriteLine($stage)
        $stage = ''
    } catch {
        if (-not [string]::IsNullOrEmpty($stage) -and [IO.Directory]::Exists($stage)) {
            Remove-HerdrRemoteTree -Path $stage
        }
        throw
    } finally {
        if ([IO.File]::Exists($Archive)) {
            [IO.File]::Delete($Archive)
        }
    }
}

function Invoke-HerdrRemoteActivateInstall {
    param(
        [Parameter(Mandatory = $true)][string] $Stage,
        [Parameter(Mandatory = $true)][string] $Destination
    )

    Assert-HerdrRemoteStagePath -Stage $Stage -Destination $Destination
    Assert-HerdrPortablePayload -Root $Stage -AllowLease
    if ([IO.Directory]::Exists($Destination)) {
        Remove-HerdrRemoteSidecar -Path $Destination
    }
    [IO.Directory]::Move($Stage, $Destination)
}
