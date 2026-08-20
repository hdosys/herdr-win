$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

function Get-RequiredEnvironmentValue([string] $Name) {
    $value = [Environment]::GetEnvironmentVariable($Name, 'Process')
    if ([string]::IsNullOrEmpty($value)) {
        throw "missing required environment value $Name"
    }
    return $value
}

$program = Get-RequiredEnvironmentValue 'HERDR_INTERACTIVE_LAUNCH_PROGRAM'
$arguments = Get-RequiredEnvironmentValue 'HERDR_INTERACTIVE_LAUNCH_ARGUMENTS'
$workingDirectory = Get-RequiredEnvironmentValue 'HERDR_INTERACTIVE_LAUNCH_WORKING_DIRECTORY'
$sessionText = Get-RequiredEnvironmentValue 'HERDR_INTERACTIVE_LAUNCH_SESSION_ID'
$sessionId = 0
if (-not [int]::TryParse(
        $sessionText,
        [Globalization.NumberStyles]::None,
        [Globalization.CultureInfo]::InvariantCulture,
        [ref] $sessionId
    ) -or $sessionId -le 0) {
    throw 'interactive launch session id is invalid'
}

$service = New-Object -ComObject 'Schedule.Service'
$service.Connect()
$root = $service.GetFolder('\')
$taskName = 'HerdrInteractiveServer-' + [Guid]::NewGuid().ToString('N')
$registered = $null
$running = $null
$deleted = $false

try {
    $definition = $service.NewTask(0)
    $definition.RegistrationInfo.Description = 'Launch the Herdr server in the existing interactive session'
    $definition.Principal.UserId = [Security.Principal.WindowsIdentity]::GetCurrent().Name
    $definition.Principal.LogonType = 3
    $definition.Principal.RunLevel = 0
    $definition.Settings.Enabled = $true
    $definition.Settings.Hidden = $true
    $definition.Settings.ExecutionTimeLimit = 'PT0S'
    $definition.Settings.DisallowStartIfOnBatteries = $false
    $definition.Settings.StopIfGoingOnBatteries = $false

    $action = $definition.Actions.Create(0)
    $action.Path = $program
    $action.Arguments = $arguments
    $action.WorkingDirectory = $workingDirectory

    $registered = $root.RegisterTaskDefinition(
        $taskName,
        $definition,
        2,
        $null,
        $null,
        3,
        $null
    )
    $running = $registered.RunEx($null, 4, $sessionId, $null)
    $processId = [int] $running.EnginePID
    if ($processId -le 0) {
        throw 'Task Scheduler started the server without returning a process id'
    }

    $root.DeleteTask($taskName, 0)
    $deleted = $true
    [Console]::Out.WriteLine("HERDR_INTERACTIVE_SERVER_PID=$processId")
} finally {
    if (-not $deleted -and $null -ne $running) {
        try { $running.Stop() } catch {}
    }
    if (-not $deleted -and $null -ne $registered) {
        try { $root.DeleteTask($taskName, 0) } catch {}
    }
}
