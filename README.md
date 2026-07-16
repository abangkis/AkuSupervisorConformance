# AkuSupervisorConformance

`AkuSupervisorConformance` is the optional compatibility laboratory for
AkuSupervisor-managed application runtimes. It is deliberately a separate
project: installing, building, and running AkuSupervisor does not require this
repository or any runtime represented by its fixtures.

The first maintained target is an application-owned Node.js HTTP server on
Windows. The fixture has no package dependencies and uses only Node.js built-in
modules. Go, Rust, Kotlin/JVM, Linux, and macOS targets can be added without
changing AkuSupervisor's Rust dependency graph or release gate.

## Boundaries

- AkuSupervisor owns generic process supervision, native signals, bounded
  fallback termination, health checks, and lifecycle evidence.
- This repository owns runtime-specific fixtures, deterministic application
  tests, native integration runners, and versioned conformance reports.
- Runtime conformance is never an implicit AkuSupervisor build or promotion
  prerequisite.
- A missing runtime is reported as `skipped`, not as a core AkuSupervisor
  failure.

See [Architecture](docs/architecture.md) and the
[conformance contract](docs/conformance-contract.md) for the durable rules.
The current per-runtime result is tracked separately in
[Validation status](docs/validation-status.md); a fixture existing in this
repository does not by itself make its recipe maintained.

## Windows Node.js validation

Prerequisites:

- Windows with PowerShell 5.1 or newer;
- Node.js 20 or newer; and
- an existing AkuSupervisor executable.

From this repository:

```powershell
.\scripts\conformance.ps1 `
  -SupervisorPath C:\WorkspaceCodex\AkuWorkspace\AkuSupervisor\target\dev\aku-supervisor.exe `
  -Runtime node
```

The command first runs the deterministic Node application test. It then starts
an isolated AkuSupervisor instance with its own configuration and dynamically
selected loopback ports, starts the fixture through the Supervisor, performs a
native stop, and writes a JSON report beneath `.artifacts/reports`.

Exit codes are stable:

| Exit code | Meaning |
|---|---|
| `0` | Every required check passed. |
| `1` | The run executed but one or more checks failed. |
| `2` | The requested runtime/platform prerequisite is unavailable, so the run was skipped. |

The runner does not modify the user's normal AkuSupervisor configuration and
does not invoke `promote-stable.ps1`.

## Fixture-only test

The application contract can be tested without AkuSupervisor:

```powershell
node .\fixtures\node-application-owned\test\application.test.mjs
```
