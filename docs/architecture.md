# Architecture

## Repository boundary

AkuSupervisorConformance is a consumer of an AkuSupervisor executable, not a
library dependency and not a submodule. A conformance invocation receives the
binary path explicitly. AkuSupervisor never searches for, downloads, or runs
this project on behalf of a normal user.

```text
AkuSupervisor release/build                 AkuSupervisorConformance
---------------------------                 ------------------------
Rust core and Rust-only tests     binary -> runtime fixture + native runner
No Node/Go/JVM dependency                   Node/Go/JVM installed only here
Generic behavior contract                   Runtime-specific evidence
```

The repositories may be checked out independently. CI that wants compatibility
evidence checks out both repositories and supplies the AkuSupervisor artifact
to the conformance runner.

## Layers

1. **Fixture manifest** declares the runtime, minimum version, supported
   platforms, entry point, health contract, and required application evidence.
2. **Deterministic application test** invokes the fixture's real shutdown
   function directly and verifies idempotency, request draining, resource
   cleanup, listener closure, and natural completion.
3. **Native integration runner** launches the same entry point through an
   isolated AkuSupervisor instance and asks the Supervisor to stop it. The
   runner verifies Supervisor, application-log, journal, port, and unrelated
   process evidence.
4. **JSON report** binds the result to the exact Supervisor binary hash,
   conformance version, runtime version, OS, fixture contract, and checks.

Application tests and native tests are intentionally separate. A deterministic
test is better at resource-level assertions, while only the native run proves
the actual platform signal path and process-tree boundary.

## Dependency policy

- Core AkuSupervisor tests must not call this repository.
- `promote-stable.ps1` must not call this repository.
- This repository may require the runtime that a selected fixture validates.
- One runtime fixture must not require another runtime. The Node fixture, for
  example, cannot require Go or an npm package install.
- Generated run data belongs under `.artifacts` and is not committed by
  default. Curated evidence may be copied into a future release record.

## Compatibility status

A recipe becomes `maintained` for one OS/runtime tuple only when its latest
compatible fixture contract passes both deterministic and native gates. Passing
on Windows does not imply Linux or macOS support. A stale or failed tuple does
not break the AkuSupervisor core release; it changes only that compatibility
claim.

