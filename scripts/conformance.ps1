[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$SupervisorPath,

    [Parameter(Mandatory = $true)]
    [ValidateSet("node", "go", "rust")]
    [string]$Runtime,

    [string]$RuntimePath,

    [string]$ReportPath,

    [string]$CargoPath
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$projectRoot = Split-Path -Parent $PSScriptRoot
$manifestPath = Join-Path $projectRoot "Cargo.toml"

if ([string]::IsNullOrWhiteSpace($CargoPath)) {
    $cargoCommand = Get-Command cargo -CommandType Application -ErrorAction SilentlyContinue |
        Select-Object -First 1
    if ($null -eq $cargoCommand) {
        throw "Cargo is required to build the Rust conformance runner. Supply -CargoPath explicitly."
    }
    $CargoPath = $cargoCommand.Source
}

if (-not (Test-Path -LiteralPath $CargoPath -PathType Leaf)) {
    throw "Cargo executable does not exist: $CargoPath"
}
$CargoPath = (Resolve-Path -LiteralPath $CargoPath).Path
$cargoDirectory = Split-Path -Parent $CargoPath
$siblingRustc = Join-Path $cargoDirectory "rustc.exe"
$siblingRustdoc = Join-Path $cargoDirectory "rustdoc.exe"
if (Test-Path -LiteralPath $siblingRustc -PathType Leaf) {
    $env:RUSTC = $siblingRustc
}
if (Test-Path -LiteralPath $siblingRustdoc -PathType Leaf) {
    $env:RUSTDOC = $siblingRustdoc
}

Write-Host "[conformance] Building the Rust runner with $CargoPath" -ForegroundColor Cyan
& $CargoPath build --manifest-path $manifestPath
if ($LASTEXITCODE -ne 0) {
    throw "Rust conformance runner build failed with exit code $LASTEXITCODE"
}

$runner = Join-Path $projectRoot "target\debug\aku-supervisor-conformance.exe"
if (-not (Test-Path -LiteralPath $runner -PathType Leaf)) {
    throw "Rust conformance runner was not produced: $runner"
}

$runnerArguments = @(
    "run",
    "--project-root", $projectRoot,
    "--supervisor", $SupervisorPath,
    "--runtime", $Runtime
)
if (-not [string]::IsNullOrWhiteSpace($RuntimePath)) {
    $runnerArguments += @("--runtime-path", $RuntimePath)
}
if (-not [string]::IsNullOrWhiteSpace($ReportPath)) {
    $runnerArguments += @("--report", $ReportPath)
}

& $runner @runnerArguments
exit $LASTEXITCODE
