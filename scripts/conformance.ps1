[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$SupervisorPath,

    [ValidateSet("node")]
    [string]$Runtime = "node",

    [string]$RuntimePath,

    [string]$ReportPath
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$script:ProjectRoot = Split-Path -Parent $PSScriptRoot
$script:StartedAt = [DateTimeOffset]::UtcNow
$script:RunId = "{0}-{1}" -f $script:StartedAt.ToString("yyyyMMddTHHmmssfffZ"), $PID
$script:Checks = New-Object System.Collections.ArrayList
$script:Evidence = [ordered]@{}
$script:SupervisorProcess = $null
$script:SentinelProcess = $null
$script:ServiceStarted = $false
$script:ServiceStopped = $false
$script:ConfigPath = $null
$script:SupervisorExecutable = $null
$script:RuntimeExecutable = $null
$script:SupervisorVersion = $null
$script:SupervisorHash = $null
$script:RuntimeVersion = $null
$script:ServicePort = $null
$script:FixtureManifest = $null
$script:FixtureManifestPath = Join-Path $script:ProjectRoot "fixtures\node-application-owned\fixture.json"
$script:ConformanceVersion = (Get-Content -LiteralPath (Join-Path $script:ProjectRoot "VERSION") -Raw).Trim()
$script:RunDirectory = Join-Path $script:ProjectRoot (".artifacts\runs\{0}" -f $script:RunId)

if ([string]::IsNullOrWhiteSpace($ReportPath)) {
    $ReportPath = Join-Path $script:ProjectRoot (".artifacts\reports\{0}.json" -f $script:RunId)
}

function Write-Utf8NoBom {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$Content
    )

    $parent = Split-Path -Parent $Path
    if (-not [string]::IsNullOrWhiteSpace($parent)) {
        New-Item -ItemType Directory -Path $parent -Force | Out-Null
    }
    $encoding = New-Object System.Text.UTF8Encoding($false)
    [System.IO.File]::WriteAllText($Path, $Content, $encoding)
}

function Add-Check {
    param(
        [Parameter(Mandatory = $true)][string]$Id,
        [Parameter(Mandatory = $true)][ValidateSet("passed", "failed", "skipped")][string]$Status,
        [Parameter(Mandatory = $true)][AllowNull()][object]$Expected,
        [Parameter(Mandatory = $true)][AllowNull()][object]$Actual,
        [Parameter(Mandatory = $true)][string]$Detail
    )

    [void]$script:Checks.Add([ordered]@{
        id = $Id
        status = $Status
        expected = $Expected
        actual = $Actual
        detail = $Detail
    })

    $color = switch ($Status) {
        "passed" { "Green" }
        "failed" { "Red" }
        default { "Yellow" }
    }
    Write-Host ("[{0}] {1}: {2}" -f $Status, $Id, $Detail) -ForegroundColor $color
}

function Get-HostOs {
    if ([Environment]::OSVersion.Platform -eq [PlatformID]::Win32NT) {
        return "windows"
    }
    return "unknown"
}

function Resolve-CommandPath {
    param([string]$ExplicitPath, [string]$CommandName)

    if (-not [string]::IsNullOrWhiteSpace($ExplicitPath)) {
        if (-not (Test-Path -LiteralPath $ExplicitPath -PathType Leaf)) {
            return $null
        }
        return (Resolve-Path -LiteralPath $ExplicitPath).Path
    }

    $command = Get-Command $CommandName -CommandType Application -ErrorAction SilentlyContinue |
        Select-Object -First 1
    if ($null -eq $command) {
        return $null
    }
    return $command.Source
}

function Get-FreeTcpPort {
    # Keep server listeners below Windows' default dynamic client-port range.
    # Selecting port 0 can return adjacent ephemeral ports, which are a poor
    # fit for a test that immediately creates loopback client connections.
    $minimum = 48000
    $count = 1000
    $start = Get-Random -Minimum 0 -Maximum $count
    for ($offset = 0; $offset -lt $count; $offset += 1) {
        $port = $minimum + (($start + $offset) % $count)
        $listener = New-Object System.Net.Sockets.TcpListener([System.Net.IPAddress]::Loopback, $port)
        try {
            $listener.Start()
            return $port
        }
        catch [System.Net.Sockets.SocketException] {
            # Try the next bounded candidate.
        }
        finally {
            $listener.Stop()
        }
    }
    throw "no free loopback port was found in the conformance range 48000-48999"
}

function Test-TcpPortOpen {
    param([int]$Port, [int]$TimeoutMs = 400)

    $client = New-Object System.Net.Sockets.TcpClient
    try {
        $pending = $client.BeginConnect("127.0.0.1", $Port, $null, $null)
        if (-not $pending.AsyncWaitHandle.WaitOne($TimeoutMs)) {
            return $false
        }
        $client.EndConnect($pending)
        return $true
    }
    catch {
        return $false
    }
    finally {
        $client.Dispose()
    }
}

function Invoke-SupervisorJson {
    param([Parameter(Mandatory = $true)][string[]]$Arguments)

    $allArguments = @($Arguments) + @("--json", "--config", $script:ConfigPath)
    $output = @(& $script:SupervisorExecutable @allArguments 2>&1)
    $exitCode = $LASTEXITCODE
    $text = ($output | ForEach-Object { $_.ToString() }) -join [Environment]::NewLine
    if ($exitCode -ne 0) {
        throw "AkuSupervisor command failed with exit code ${exitCode}: $text"
    }
    try {
        return $text | ConvertFrom-Json
    }
    catch {
        throw "AkuSupervisor returned invalid JSON: $text"
    }
}

function Wait-SupervisorReady {
    $deadline = [DateTimeOffset]::UtcNow.AddSeconds(10)
    while ([DateTimeOffset]::UtcNow -lt $deadline) {
        if ($script:SupervisorProcess.HasExited) {
            throw "AkuSupervisor exited before its control API became ready"
        }
        try {
            return Invoke-SupervisorJson -Arguments @("status")
        }
        catch {
            Start-Sleep -Milliseconds 100
        }
    }
    throw "AkuSupervisor control API did not become ready within 10 seconds"
}

function Wait-ApplicationHealth {
    param([int]$Port, [string]$Path)

    $deadline = [DateTimeOffset]::UtcNow.AddSeconds(10)
    $lastError = $null
    while ([DateTimeOffset]::UtcNow -lt $deadline) {
        try {
            $response = Invoke-WebRequest -UseBasicParsing `
                -Uri "http://127.0.0.1:${Port}${Path}" -TimeoutSec 2
            if ($response.StatusCode -eq 200) {
                return $response.StatusCode
            }
            $lastError = "HTTP status $($response.StatusCode)"
        }
        catch {
            $lastError = $_.Exception.Message
        }
        Start-Sleep -Milliseconds 100
    }
    throw "fixture application health did not become ready: $lastError"
}

function Get-ProcessThreadDiagnostic {
    param([int]$ProcessId)

    try {
        $process = Get-Process -Id $ProcessId -ErrorAction Stop
        $commandLine = $null
        $executablePath = $null
        try {
            $processRecord = Get-CimInstance Win32_Process -Filter "ProcessId = $ProcessId" `
                -ErrorAction Stop
            $commandLine = $processRecord.CommandLine
            $executablePath = $processRecord.ExecutablePath
        }
        catch {}
        $threads = @($process.Threads | ForEach-Object {
            $thread = $_
            $waitReason = $null
            $startTimeUtc = $null
            $totalProcessorTimeMs = $null
            try { $waitReason = $thread.WaitReason.ToString() } catch {}
            try { $startTimeUtc = $thread.StartTime.ToUniversalTime().ToString("o") } catch {}
            try { $totalProcessorTimeMs = [int64]$thread.TotalProcessorTime.TotalMilliseconds } catch {}
            [ordered]@{
                id = $thread.Id
                state = $thread.ThreadState.ToString()
                waitReason = $waitReason
                startTimeUtc = $startTimeUtc
                totalProcessorTimeMs = $totalProcessorTimeMs
            }
        })
        return [ordered]@{
            processId = $ProcessId
            executablePath = $executablePath
            commandLine = $commandLine
            totalProcessorTimeMs = [int64]$process.TotalProcessorTime.TotalMilliseconds
            threads = $threads
        }
    }
    catch {
        return [ordered]@{
            processId = $ProcessId
            error = $_.Exception.Message
        }
    }
}

function Start-IsolatedSupervisor {
    $startInfo = New-Object System.Diagnostics.ProcessStartInfo
    $startInfo.FileName = $script:SupervisorExecutable
    $startInfo.Arguments = 'run --config "{0}"' -f $script:ConfigPath
    $startInfo.WorkingDirectory = $script:RunDirectory
    $startInfo.UseShellExecute = $false
    $startInfo.RedirectStandardInput = $true
    $startInfo.CreateNoWindow = $false
    $process = New-Object System.Diagnostics.Process
    $process.StartInfo = $startInfo
    if (-not $process.Start()) {
        throw "failed to start isolated AkuSupervisor"
    }
    return $process
}

function Start-UnrelatedSentinel {
    $startInfo = New-Object System.Diagnostics.ProcessStartInfo
    $startInfo.FileName = $script:RuntimeExecutable
    $startInfo.Arguments = '-e "setInterval(() => {}, 1000)"'
    $startInfo.UseShellExecute = $false
    $startInfo.CreateNoWindow = $true
    $startInfo.RedirectStandardOutput = $true
    $startInfo.RedirectStandardError = $true
    $process = New-Object System.Diagnostics.Process
    $process.StartInfo = $startInfo
    if (-not $process.Start()) {
        throw "failed to start unrelated sentinel process"
    }
    Start-Sleep -Milliseconds 100
    if ($process.HasExited) {
        throw "unrelated sentinel exited during startup"
    }
    return $process
}

function Read-ApplicationRecords {
    $logs = Invoke-SupervisorJson -Arguments @(
        "logs", "node-application-owned", "--stream", "stdout", "--tail", "200"
    )
    $lines = @($logs.response.log.lines)
    $records = New-Object System.Collections.ArrayList
    foreach ($line in $lines) {
        try {
            [void]$records.Add(($line | ConvertFrom-Json))
        }
        catch {
            # Non-JSON output is retained in raw evidence but is not an event.
        }
    }
    return [ordered]@{ lines = $lines; records = @($records) }
}

function Write-Report {
    param(
        [Parameter(Mandatory = $true)][ValidateSet("passed", "failed", "skipped")][string]$Status,
        [Parameter(Mandatory = $true)][ValidateSet(0, 1, 2)][int]$ExitCode
    )

    $fixtureId = "node-application-owned"
    $contractVersion = 1
    if ($null -ne $script:FixtureManifest) {
        $fixtureId = $script:FixtureManifest.id
        $contractVersion = $script:FixtureManifest.contractVersion
    }

    $report = [ordered]@{
        schemaVersion = 1
        conformanceVersion = $script:ConformanceVersion
        runId = $script:RunId
        startedAtUtc = $script:StartedAt.ToString("o")
        completedAtUtc = [DateTimeOffset]::UtcNow.ToString("o")
        status = $Status
        exitCode = $ExitCode
        supervisor = [ordered]@{
            path = if ($null -eq $script:SupervisorExecutable) { $SupervisorPath } else { $script:SupervisorExecutable }
            version = $script:SupervisorVersion
            sha256 = $script:SupervisorHash
        }
        host = [ordered]@{
            os = Get-HostOs
            architecture = [Environment]::GetEnvironmentVariable("PROCESSOR_ARCHITECTURE")
            powershellVersion = $PSVersionTable.PSVersion.ToString()
        }
        runtime = [ordered]@{
            id = $Runtime
            path = $script:RuntimeExecutable
            version = $script:RuntimeVersion
        }
        fixture = [ordered]@{
            id = $fixtureId
            contractVersion = $contractVersion
            manifestPath = $script:FixtureManifestPath
        }
        checks = @($script:Checks)
        evidence = $script:Evidence
    }

    Write-Utf8NoBom -Path $ReportPath -Content ($report | ConvertTo-Json -Depth 30)
    Write-Host "Conformance report: $ReportPath" -ForegroundColor Cyan
}

New-Item -ItemType Directory -Path $script:RunDirectory -Force | Out-Null

if ((Get-HostOs) -ne "windows") {
    Add-Check -Id "platform_supported" -Status "skipped" -Expected "windows" -Actual (Get-HostOs) `
        -Detail "the initial native runner supports Windows only"
    Write-Report -Status "skipped" -ExitCode 2
    exit 2
}

if (-not (Test-Path -LiteralPath $SupervisorPath -PathType Leaf)) {
    Add-Check -Id "supervisor_available" -Status "failed" -Expected "existing executable" -Actual $SupervisorPath `
        -Detail "the supplied AkuSupervisor executable does not exist"
    Write-Report -Status "failed" -ExitCode 1
    exit 1
}

$script:SupervisorExecutable = (Resolve-Path -LiteralPath $SupervisorPath).Path
$script:RuntimeExecutable = Resolve-CommandPath -ExplicitPath $RuntimePath -CommandName "node"
if ($null -eq $script:RuntimeExecutable) {
    Add-Check -Id "runtime_available" -Status "skipped" -Expected "Node.js >=20" -Actual $null `
        -Detail "Node.js is not available; AkuSupervisor itself remains unaffected"
    Write-Report -Status "skipped" -ExitCode 2
    exit 2
}

$script:FixtureManifest = Get-Content -LiteralPath $script:FixtureManifestPath -Raw | ConvertFrom-Json
$script:SupervisorVersion = ((& $script:SupervisorExecutable --version 2>&1) | Out-String).Trim()
$script:SupervisorHash = (Get-FileHash -LiteralPath $script:SupervisorExecutable -Algorithm SHA256).Hash.ToLowerInvariant()
$script:RuntimeVersion = ((& $script:RuntimeExecutable --version 2>&1) | Out-String).Trim()

$normalizedRuntimeVersion = $script:RuntimeVersion.TrimStart("v").Split("-")[0]
$minimumRuntimeVersion = [Version]$script:FixtureManifest.runtime.minimumVersion
if ([Version]$normalizedRuntimeVersion -lt $minimumRuntimeVersion) {
    Add-Check -Id "runtime_version" -Status "skipped" -Expected (">= {0}" -f $minimumRuntimeVersion) `
        -Actual $script:RuntimeVersion -Detail "the selected Node.js version is below the fixture minimum"
    Write-Report -Status "skipped" -ExitCode 2
    exit 2
}
Add-Check -Id "runtime_version" -Status "passed" -Expected (">= {0}" -f $minimumRuntimeVersion) `
    -Actual $script:RuntimeVersion -Detail "runtime prerequisite is available only for this opt-in conformance run"

$fixtureRoot = Split-Path -Parent $script:FixtureManifestPath
$entrypoint = Join-Path $fixtureRoot $script:FixtureManifest.service.entrypoint
$testPath = Join-Path $fixtureRoot "test\application.test.mjs"
$controlPort = Get-FreeTcpPort
$servicePort = Get-FreeTcpPort
while ($servicePort -eq $controlPort) {
    $servicePort = Get-FreeTcpPort
}
$script:ServicePort = $servicePort

$script:ConfigPath = Join-Path $script:RunDirectory "services.json"
$config = [ordered]@{
    version = 1
    control = [ordered]@{
        host = "127.0.0.1"
        port = $controlPort
        tokenFile = ".runtime\$($script:RunId)\control-token"
        mcp = [ordered]@{ enabled = $false; allowedOrigins = @() }
    }
    observability = [ordered]@{ consoleEvents = "verbose" }
    services = [ordered]@{
        "node-application-owned" = [ordered]@{
            label = "Node application-owned conformance fixture"
            cwd = $fixtureRoot
            command = $script:RuntimeExecutable
            args = @(
                $entrypoint,
                "--host", "127.0.0.1",
                "--port", $servicePort.ToString(),
                "--shutdown-ms", $script:FixtureManifest.service.applicationShutdownMs.ToString()
            )
            environment = [ordered]@{}
            health = [ordered]@{ type = "process" }
            ports = @($servicePort)
            restartPolicy = "manual"
            shutdownGraceMs = $script:FixtureManifest.service.supervisorShutdownGraceMs
        }
    }
}
Write-Utf8NoBom -Path $script:ConfigPath -Content ($config | ConvertTo-Json -Depth 20)

$exitCode = 1
$finalStatus = "failed"
try {
    # Executing the node:test file directly keeps this fixture dependency-free
    # and avoids the test runner's optional child-process isolation.
    $deterministicOutput = @(& $script:RuntimeExecutable $testPath 2>&1)
    $deterministicExit = $LASTEXITCODE
    $deterministicText = ($deterministicOutput | ForEach-Object { $_.ToString() }) -join [Environment]::NewLine
    Write-Utf8NoBom -Path (Join-Path $script:RunDirectory "deterministic-test.log") -Content $deterministicText
    $script:Evidence.deterministicTestLog = Join-Path $script:RunDirectory "deterministic-test.log"
    if ($deterministicExit -ne 0) {
        Add-Check -Id "deterministic_application_test" -Status "failed" -Expected 0 -Actual $deterministicExit `
            -Detail "the fixture application test failed"
        throw "deterministic Node fixture test failed"
    }
    Add-Check -Id "deterministic_application_test" -Status "passed" -Expected 0 -Actual $deterministicExit `
        -Detail "idempotency, active-request drain, resource cleanup, and listener release passed"

    $script:SentinelProcess = Start-UnrelatedSentinel
    $script:Evidence.unrelatedSentinelPid = $script:SentinelProcess.Id

    $script:SupervisorProcess = Start-IsolatedSupervisor
    $script:Evidence.supervisorPid = $script:SupervisorProcess.Id
    $ready = Wait-SupervisorReady
    Add-Check -Id "isolated_supervisor_ready" -Status "passed" -Expected $controlPort `
        -Actual $controlPort -Detail "isolated control API is ready on a run-specific configuration"

    $startReason = "AkuSupervisorConformance native Node start $($script:RunId)"
    # Mark the service as cleanup-eligible before the call because a failed
    # readiness contract can still leave an owned, unhealthy process running.
    $script:ServiceStarted = $true
    $start = Invoke-SupervisorJson -Arguments @(
        "start", "node-application-owned",
        "--actor", "codex",
        "--reason", $startReason,
        "--request-id", "conformance-start-$($script:RunId)"
    )
    $script:Evidence.startResponse = $start.response
    $startPassed = $start.response.outcome -eq "started"
    Add-Check -Id "supervised_start" -Status $(if ($startPassed) { "passed" } else { "failed" }) `
        -Expected "started" -Actual $start.response.outcome -Detail "AkuSupervisor owns the direct Node executable"

    $runningStatus = Invoke-SupervisorJson -Arguments @("status")
    $serviceStatus = @($runningStatus.response.services | Where-Object { $_.id -eq "node-application-owned" })[0]
    $processHealthPassed = $serviceStatus.health.status -eq "healthy"
    Add-Check -Id "supervisor_process_health" -Status $(if ($processHealthPassed) { "passed" } else { "failed" }) `
        -Expected "healthy" -Actual $serviceStatus.health.status `
        -Detail "the shutdown suite isolates generic process ownership from HTTP-adapter conformance"

    try {
        $applicationHealthStatus = Wait-ApplicationHealth -Port $servicePort `
            -Path $script:FixtureManifest.service.healthPath
        Add-Check -Id "application_health_reached" -Status "passed" -Expected 200 `
            -Actual $applicationHealthStatus -Detail "the runner independently reached the fixture readiness endpoint"
    }
    catch {
        $script:Evidence.applicationHealthFailureProcess = Get-ProcessThreadDiagnostic `
            -ProcessId $serviceStatus.rootPid
        Add-Check -Id "application_health_reached" -Status "failed" -Expected 200 `
            -Actual $_.Exception.Message `
            -Detail "the owned process existed, but its application entrypoint did not become reachable"
    }

    $stopReason = "AkuSupervisorConformance native Node stop $($script:RunId)"
    $stop = Invoke-SupervisorJson -Arguments @(
        "stop", "node-application-owned",
        "--actor", "codex",
        "--reason", $stopReason,
        "--request-id", "conformance-stop-$($script:RunId)"
    )
    $script:ServiceStopped = $true
    $script:Evidence.stopResponse = $stop.response

    $shutdown = $stop.response.shutdown
    $signalSent = $shutdown.gracefulSignalSent -eq $true
    Add-Check -Id "graceful_signal_sent" -Status $(if ($signalSent) { "passed" } else { "failed" }) `
        -Expected $true -Actual $shutdown.gracefulSignalSent -Detail "AkuSupervisor reported targeted native signal delivery"

    $notForced = $shutdown.forced -eq $false
    Add-Check -Id "no_forced_fallback" -Status $(if ($notForced) { "passed" } else { "failed" }) `
        -Expected $false -Actual $shutdown.forced -Detail "the Node process exited before the Supervisor fallback deadline"

    $ownedAfter = @($shutdown.ownedPidsAfter)
    $treeEmpty = $ownedAfter.Count -eq 0
    Add-Check -Id "owned_tree_empty" -Status $(if ($treeEmpty) { "passed" } else { "failed" }) `
        -Expected @() -Actual $ownedAfter -Detail "no managed descendant survived the stop"

    $applicationEvidence = Read-ApplicationRecords
    $script:Evidence.applicationStdout = $applicationEvidence.lines
    $applicationRecords = @($applicationEvidence.records)
    $observedEvents = @($applicationRecords | ForEach-Object { $_.event })
    $missingEvents = @($script:FixtureManifest.expectedEvidence.applicationEvents |
        Where-Object { $observedEvents -notcontains $_ })
    $eventsPassed = $missingEvents.Count -eq 0
    Add-Check -Id "application_cleanup_events" -Status $(if ($eventsPassed) { "passed" } else { "failed" }) `
        -Expected @($script:FixtureManifest.expectedEvidence.applicationEvents) -Actual $observedEvents `
        -Detail $(if ($eventsPassed) { "every required application event was logged" } else { "required application events are missing" })

    $shutdownStarted = @($applicationRecords | Where-Object { $_.event -eq "shutdown_started" } | Select-Object -Last 1)
    $observedSignal = if ($shutdownStarted.Count -eq 0) { $null } else { $shutdownStarted[0].signal }
    $signalPassed = $observedSignal -eq $script:FixtureManifest.expectedEvidence.windowsSignal
    Add-Check -Id "application_observed_native_signal" -Status $(if ($signalPassed) { "passed" } else { "failed" }) `
        -Expected $script:FixtureManifest.expectedEvidence.windowsSignal -Actual $observedSignal `
        -Detail "the application handler, not only the process tree, recorded the Windows signal"

    $events = Invoke-SupervisorJson -Arguments @("events", "--limit", "200")
    $journalRecord = @($events.response.events | Where-Object {
        $_.serviceId -eq "node-application-owned" -and
        $_.action -eq "stop" -and
        $_.reason -eq $stopReason
    } | Select-Object -Last 1)
    $journalPassed = $journalRecord.Count -eq 1 -and
        $journalRecord[0].shutdown.gracefulSignalSent -eq $true -and
        $journalRecord[0].shutdown.forced -eq $false -and
        @($journalRecord[0].shutdown.ownedPidsAfter).Count -eq 0
    $script:Evidence.lifecycleRecord = if ($journalRecord.Count -eq 1) { $journalRecord[0] } else { $null }
    Add-Check -Id "lifecycle_journal_matches" -Status $(if ($journalPassed) { "passed" } else { "failed" }) `
        -Expected "graceful=true, forced=false, ownedPidsAfter=[]" `
        -Actual $(if ($journalRecord.Count -eq 1) { $journalRecord[0].shutdown } else { $null }) `
        -Detail "the canonical journal carries the same shutdown evidence"

    Start-Sleep -Milliseconds 100
    $portReleased = -not (Test-TcpPortOpen -Port $servicePort)
    Add-Check -Id "listener_port_released" -Status $(if ($portReleased) { "passed" } else { "failed" }) `
        -Expected "closed" -Actual $(if ($portReleased) { "closed" } else { "open" }) `
        -Detail "the declared fixture listener is no longer reachable"

    $sentinelAlive = -not $script:SentinelProcess.HasExited
    Add-Check -Id "unrelated_process_preserved" -Status $(if ($sentinelAlive) { "passed" } else { "failed" }) `
        -Expected "running" -Actual $(if ($sentinelAlive) { "running" } else { "exited" }) `
        -Detail "a process outside the Supervisor ownership boundary was not affected"
}
catch {
    if ($script:ServiceStarted -and $null -ne $script:ServicePort) {
        $diagnostic = [ordered]@{
            tcpOpen = Test-TcpPortOpen -Port $script:ServicePort -TimeoutMs 1000
            httpStatus = $null
            httpError = $null
        }
        try {
            $response = Invoke-WebRequest -UseBasicParsing `
                -Uri "http://127.0.0.1:$($script:ServicePort)/health" -TimeoutSec 3
            $diagnostic.httpStatus = $response.StatusCode
        }
        catch {
            $diagnostic.httpError = $_.Exception.Message
        }
        $script:Evidence.failureReadinessDiagnostic = $diagnostic
    }
    Add-Check -Id "runner_completed" -Status "failed" -Expected "completed" -Actual $_.Exception.Message `
        -Detail "the native conformance runner encountered an error"
}
finally {
    if ($script:ServiceStarted -and -not $script:ServiceStopped -and $null -ne $script:SupervisorProcess -and -not $script:SupervisorProcess.HasExited) {
        try {
            $null = Invoke-SupervisorJson -Arguments @(
                "stop", "node-application-owned",
                "--actor", "codex",
                "--reason", "AkuSupervisorConformance failure cleanup $($script:RunId)",
                "--request-id", "conformance-cleanup-$($script:RunId)"
            )
        }
        catch {
            Write-Warning "fixture cleanup through AkuSupervisor failed: $($_.Exception.Message)"
        }
    }

    if ($null -ne $script:SupervisorProcess -and -not $script:SupervisorProcess.HasExited) {
        try {
            $script:SupervisorProcess.StandardInput.WriteLine("quit")
            $script:SupervisorProcess.StandardInput.Flush()
            $script:SupervisorProcess.StandardInput.Close()
            if (-not $script:SupervisorProcess.WaitForExit(5000)) {
                $script:SupervisorProcess.Kill()
                $script:SupervisorProcess.WaitForExit()
            }
        }
        catch {
            Write-Warning "isolated Supervisor cleanup failed: $($_.Exception.Message)"
        }
    }

    if ($null -ne $script:SentinelProcess -and -not $script:SentinelProcess.HasExited) {
        try {
            $script:SentinelProcess.Kill()
            $script:SentinelProcess.WaitForExit()
        }
        catch {
            Write-Warning "unrelated sentinel cleanup failed: $($_.Exception.Message)"
        }
    }
}

$failedChecks = @($script:Checks | Where-Object { $_.status -eq "failed" })
if ($failedChecks.Count -eq 0) {
    $finalStatus = "passed"
    $exitCode = 0
}
Write-Report -Status $finalStatus -ExitCode $exitCode
exit $exitCode
