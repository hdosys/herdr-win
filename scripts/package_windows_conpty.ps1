param(
    [Parameter(Mandatory = $true)]
    [string]$HerdrExe,

    [Parameter(Mandatory = $true)]
    [string]$PackagePath,

    [Parameter(Mandatory = $true)]
    [string]$StageDir,

    [Parameter(Mandatory = $true)]
    [string]$OutputPath
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$nativeWindowsPowerShell = Join-Path $env:SystemRoot "System32\WindowsPowerShell\v1.0\powershell.exe"
$nativeWindowsPowerShellHome = Split-Path -Parent $nativeWindowsPowerShell
[string[]]$nativeModuleRoots = @(
    (Join-Path $nativeWindowsPowerShellHome "Modules"),
    (Join-Path $env:ProgramFiles "WindowsPowerShell\Modules"),
    (Join-Path ([Environment]::GetFolderPath("MyDocuments")) "WindowsPowerShell\Modules")
)
$env:PSModulePath = [string]::Join([IO.Path]::PathSeparator, $nativeModuleRoots)

if ($PSVersionTable.PSEdition -ne "Desktop" -or $PSVersionTable.PSVersion.Major -ne 5) {
    if (-not (Test-Path -LiteralPath $nativeWindowsPowerShell -PathType Leaf)) {
        throw "Windows PowerShell 5.1 was not found at $nativeWindowsPowerShell"
    }
    [string[]]$childArguments = @(
        "-NoLogo",
        "-NoProfile",
        "-NonInteractive",
        "-ExecutionPolicy", "Bypass",
        "-File", $PSCommandPath,
        "-HerdrExe", $HerdrExe,
        "-PackagePath", $PackagePath,
        "-StageDir", $StageDir,
        "-OutputPath", $OutputPath
    )
    & $nativeWindowsPowerShell @childArguments
    $childExitCode = $LASTEXITCODE
    if ($childExitCode -ne 0) {
        throw "Windows PowerShell 5.1 packaging failed with exit code $childExitCode"
    }
    return
}

$securityModule = Join-Path $nativeWindowsPowerShellHome "Modules\Microsoft.PowerShell.Security\Microsoft.PowerShell.Security.psd1"
Import-Module -Name $securityModule -ErrorAction Stop

function Invoke-NativeChecked {
    param(
        [string]$Command,
        [string[]]$Arguments
    )

    & $Command @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "$Command failed with exit code $LASTEXITCODE"
    }
}

$packager = Join-Path $PSScriptRoot "package_windows_conpty.py"
Invoke-NativeChecked python @(
    $packager,
    "stage",
    "--package", $PackagePath,
    "--herdr-exe", $HerdrExe,
    "--output-dir", $StageDir
)
Invoke-NativeChecked dotnet @("nuget", "verify", "--all", $PackagePath)

foreach ($relative in @("conpty\conpty.dll", "conpty\x64\OpenConsole.exe", "conpty\arm64\OpenConsole.exe")) {
    $signature = Get-AuthenticodeSignature (Join-Path $StageDir $relative)
    $subject = if ($null -eq $signature.SignerCertificate) { "" } else { $signature.SignerCertificate.Subject }
    if ($signature.Status -ne "Valid" -or $subject -notlike "*Microsoft Corporation*") {
        throw "Invalid Microsoft signature for $relative`: $($signature.Status) $subject"
    }
}

Invoke-NativeChecked python @(
    $packager,
    "archive",
    "--stage-dir", $StageDir,
    "--output", $OutputPath
)
