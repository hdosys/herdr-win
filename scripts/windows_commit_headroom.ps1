[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [UInt64]$MinimumHeadroomBytes
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;

public static class HerdrWinCommitSnapshot
{
    [StructLayout(LayoutKind.Sequential)]
    public struct PerformanceInformation
    {
        public UInt32 Size;
        public UIntPtr CommitTotal;
        public UIntPtr CommitLimit;
        public UIntPtr CommitPeak;
        public UIntPtr PhysicalTotal;
        public UIntPtr PhysicalAvailable;
        public UIntPtr SystemCache;
        public UIntPtr KernelTotal;
        public UIntPtr KernelPaged;
        public UIntPtr KernelNonpaged;
        public UIntPtr PageSize;
        public UInt32 HandleCount;
        public UInt32 ProcessCount;
        public UInt32 ThreadCount;
    }

    [DllImport("psapi.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static extern bool GetPerformanceInfo(
        ref PerformanceInformation information,
        UInt32 size);
}
'@

$information = New-Object HerdrWinCommitSnapshot+PerformanceInformation
$information.Size = [UInt32][Runtime.InteropServices.Marshal]::SizeOf($information)
if (-not [HerdrWinCommitSnapshot]::GetPerformanceInfo(
    [ref]$information,
    $information.Size
)) {
    $errorCode = [Runtime.InteropServices.Marshal]::GetLastWin32Error()
    throw "GetPerformanceInfo failed with Win32 error $errorCode."
}

[UInt64]$pageSize = $information.PageSize.ToUInt64()
[UInt64]$committedBytes = $information.CommitTotal.ToUInt64() * $pageSize
[UInt64]$commitLimitBytes = $information.CommitLimit.ToUInt64() * $pageSize
if ($commitLimitBytes -lt $committedBytes) {
    throw "Windows reported committed bytes above the commit limit."
}
[UInt64]$headroomBytes = $commitLimitBytes - $committedBytes

$committedGiB = "{0:F2}" -f ($committedBytes / 1GB)
$commitLimitGiB = "{0:F2}" -f ($commitLimitBytes / 1GB)
$headroomGiB = "{0:F2}" -f ($headroomBytes / 1GB)
Write-Output "commit_charge_gib=$committedGiB"
Write-Output "commit_limit_gib=$commitLimitGiB"
Write-Output "commit_headroom_gib=$headroomGiB"
if ($headroomBytes -gt $MinimumHeadroomBytes) {
    Write-Output "commit_headroom_preflight=pass"
    return
}

$privateBytesByName = @{}
$processCountByName = @{}
foreach ($process in @(Get-Process -ErrorAction SilentlyContinue)) {
    try {
        $name = [string]$process.ProcessName
        [UInt64]$privateBytes = $process.PrivateMemorySize64
    } catch {
        continue
    }
    if ([string]::IsNullOrWhiteSpace($name) -or $privateBytes -eq 0) {
        continue
    }
    if (-not $privateBytesByName.ContainsKey($name)) {
        $privateBytesByName[$name] = [UInt64]0
        $processCountByName[$name] = 0
    }
    $privateBytesByName[$name] = [UInt64]$privateBytesByName[$name] + $privateBytes
    $processCountByName[$name] = [int]$processCountByName[$name] + 1
}

$largestProcessClasses = @(
    $privateBytesByName.GetEnumerator() |
        Sort-Object -Property Value -Descending |
        Select-Object -First 5 |
        ForEach-Object {
            [ordered]@{
                name = [string]$_.Key
                count = [int]$processCountByName[$_.Key]
                private_bytes = [UInt64]$_.Value
            }
        }
)
$classSummary = @(
    $largestProcessClasses | ForEach-Object {
        $privateGiB = "{0:F2}" -f ([UInt64]$_['private_bytes'] / 1GB)
        "$($_['name']) x$($_['count']) ($privateGiB GiB private)"
    }
) -join ", "
if ([string]::IsNullOrWhiteSpace($classSummary)) {
    $classSummary = "none available"
}
$minimumGiB = "{0:F2}" -f ($MinimumHeadroomBytes / 1GB)
throw "Windows commit headroom $headroomGiB GiB is at or below the measured unsafe $minimumGiB GiB Cargo boundary (committed $committedGiB of $commitLimitGiB GiB). Largest process classes: $classSummary. Close only known idle processes or increase the pagefile, then retry; no process was terminated."
