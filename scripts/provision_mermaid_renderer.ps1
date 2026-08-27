[CmdletBinding()]
param(
    [string]$ToolRoot = (Join-Path $env:ProgramData 'HerdrSandbox\tools\mermaid-cli'),
    [ValidateSet('Process', 'User', 'Machine')]
    [string]$EnvironmentTarget = 'Machine'
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 2.0

$null = Get-Command node -ErrorAction Stop
$npm = Get-Command npm -ErrorAction Stop
$edge = @(
    (Join-Path ${env:ProgramFiles(x86)} 'Microsoft\Edge\Application\msedge.exe'),
    (Join-Path $env:ProgramFiles 'Microsoft\Edge\Application\msedge.exe')
) | Where-Object { Test-Path -LiteralPath $_ -PathType Leaf } | Select-Object -First 1
if ([string]::IsNullOrWhiteSpace($edge)) {
    throw 'Microsoft Edge is required for the maintained Mermaid renderer.'
}

$packagePath = Join-Path $ToolRoot 'node_modules\@mermaid-js\mermaid-cli\package.json'
$latestVersion = (& $npm.Source view '@mermaid-js/mermaid-cli' version --json).Trim().Trim('"')
if ($LASTEXITCODE -ne 0 -or $latestVersion -notmatch '^\d+\.\d+\.\d+$') {
    throw 'Could not resolve the latest stable Mermaid CLI release.'
}
$installedVersion = if (Test-Path -LiteralPath $packagePath -PathType Leaf) {
    (Get-Content -LiteralPath $packagePath -Raw | ConvertFrom-Json).version
} else {
    $null
}
if ($installedVersion -ne $latestVersion) {
    New-Item -ItemType Directory -Path $ToolRoot -Force | Out-Null
    $previousSkipDownload = $env:PUPPETEER_SKIP_DOWNLOAD
    try {
        $env:PUPPETEER_SKIP_DOWNLOAD = '1'
        & $npm.Source install --prefix $ToolRoot --no-audit --no-fund --save=false "@mermaid-js/mermaid-cli@$latestVersion"
        if ($LASTEXITCODE -ne 0) {
            throw "Mermaid CLI installation failed with exit code $LASTEXITCODE."
        }
    } finally {
        if ($null -eq $previousSkipDownload) {
            Remove-Item Env:PUPPETEER_SKIP_DOWNLOAD -ErrorAction SilentlyContinue
        } else {
            $env:PUPPETEER_SKIP_DOWNLOAD = $previousSkipDownload
        }
    }
}

$mmdc = Join-Path $ToolRoot 'node_modules\.bin\mmdc.cmd'
if (-not (Test-Path -LiteralPath $mmdc -PathType Leaf)) {
    throw "Mermaid CLI entrypoint is missing after installation: $mmdc"
}
$puppeteerConfig = Join-Path $ToolRoot 'puppeteer-edge.json'
@{ executablePath = $edge } | ConvertTo-Json | Set-Content -LiteralPath $puppeteerConfig -Encoding utf8
$mermaidCommand = '"{0}" --puppeteerConfigFile "{1}"' -f $mmdc, $puppeteerConfig
$env:MERMAID_CLI = $mermaidCommand
[Environment]::SetEnvironmentVariable('MERMAID_CLI', $mermaidCommand, $EnvironmentTarget)

"mermaid_version=$latestVersion"
"mermaid_cli=$mermaidCommand"
"edge=$edge"
