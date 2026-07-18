# AkuSupervisorConformance

`AkuSupervisorConformance` is the optional Rust compatibility laboratory for
AkuSupervisor-managed application runtimes. It is deliberately a separate
project: installing, building, and running AkuSupervisor does not require this
repository or any runtime represented by its fixtures.

The `v0.7.0-preview.1` tag marks the conformance state paired with the same
AkuWorkspace preview checkpoint. This laboratory is not part of the
`0.7.0-preview.1` end-user bundle.

The manifest-driven native runner is written in Rust. Node.js, Go, and managed
Rust applications are opt-in fixture dependencies, not implementation
languages of the conformance engine and not dependencies of AkuSupervisor.
All three application-owned fixtures are maintained for the current Windows
adapter after passing the same deterministic and native gates.

## Boundaries

- AkuSupervisor owns generic process supervision, native signals, bounded
  fallback termination, health checks, and lifecycle evidence.
- This repository's Rust core owns manifest loading, runtime adapters, the
  shared native integration gate, and versioned conformance reports.
- Runtime-specific fixtures own only their application implementation and
  deterministic shutdown tests.
- Runtime conformance is never an implicit AkuSupervisor build or promotion
  prerequisite.
- A missing runtime is reported as `skipped`, not as a core AkuSupervisor
  failure.

See [Architecture](docs/architecture.md) and the
[conformance contract](docs/conformance-contract.md) for the durable rules.
The current per-runtime result is tracked separately in
[Validation status](docs/validation-status.md); a fixture existing in this
repository does not by itself make its recipe maintained.

## Windows validation

Prerequisites:

- Windows with PowerShell 5.1 or newer for the thin launcher;
- Rust/Cargo 1.97 or newer for the conformance runner;
- an existing AkuSupervisor executable.

The selected fixture additionally requires its runtime: Node.js 20+, Go 1.21+,
or Rust/Cargo 1.97+.

From this repository:

```powershell
.\scripts\conformance.ps1 `
  -SupervisorPath C:\WorkspaceCodex\AkuWorkspace\AkuSupervisor\target\dev\aku-supervisor.exe `
  -Runtime node `
  -CargoPath C:\path\to\cargo.exe
```

Replace `node` with `go` or `rust`. Use `-RuntimePath` when the selected runtime
is not reliably discoverable on `PATH`; for the Rust fixture it identifies the
Cargo executable used to build the independent managed application.

The PowerShell file only builds and invokes the Rust runner. The Rust binary
then runs the deterministic application test, starts an isolated AkuSupervisor
with dynamically selected loopback ports, performs the native lifecycle gate,
and writes a JSON report beneath `.artifacts/reports`.

Exit codes are stable:

| Exit code | Meaning |
|---|---|
| `0` | Every required check passed. |
| `1` | The run executed but one or more checks failed. |
| `2` | The requested runtime/platform prerequisite is unavailable, so the run was skipped. |

The runner does not modify the user's normal AkuSupervisor configuration and
does not invoke `promote-stable.ps1`.

## Fixture-only tests

The application contract can be tested without AkuSupervisor:

```powershell
node .\fixtures\node-application-owned\test\application.test.mjs
Push-Location .\fixtures\go-application-owned
go test ./...
Pop-Location
cargo test --manifest-path .\fixtures\rust-application-owned\Cargo.toml
```
