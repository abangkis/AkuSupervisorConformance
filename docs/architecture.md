# Architecture

## Repository boundary

AkuSupervisorConformance is a consumer of an AkuSupervisor executable, not a
library dependency and not a submodule. A conformance invocation receives the
binary path explicitly. AkuSupervisor never searches for, downloads, or runs
this project on behalf of a normal user.

```text
AkuSupervisor release/build           AkuSupervisorConformance
---------------------------           ------------------------
Rust lifecycle core        binary ->  Rust native gate and report engine
No fixture runtimes                   Rust runtime-adapter modules
Generic behavior contract             Node / Go / Rust application fixtures
```

The repositories may be checked out independently. CI that wants compatibility
evidence checks out both repositories and supplies the AkuSupervisor artifact
to the conformance runner.

## Layers

1. **Rust manifest loader** discovers exactly one strict fixture contract for
   the selected runtime and rejects ambiguous or malformed manifests.
2. **Fixture manifest** declares the runtime, minimum version, supported
   platforms, entry point, health contract, and required application evidence.
3. **Deterministic application test** invokes the fixture's real shutdown
   function directly and verifies idempotency, request draining, resource
   cleanup, listener closure, and natural completion.
4. **Shared Rust native gate** launches the same entry point through an
   isolated AkuSupervisor instance and asks the Supervisor to stop it. The
   runner verifies Supervisor, application-log, journal, port, and unrelated
   process evidence.
5. **JSON report** binds the result to the exact Supervisor binary hash,
   conformance version, runtime version, OS, fixture contract, and checks.

Application tests and native tests are intentionally separate. A deterministic
test is better at resource-level assertions, while only the native run proves
the actual platform signal path and process-tree boundary.

## Dependency policy

- Core AkuSupervisor tests must not call this repository.
- `promote-stable.ps1` must not call this repository.
- This repository may require the runtime that a selected fixture validates.
- PowerShell is a thin Windows launcher only; lifecycle assertions and report
  construction must remain in the Rust binary.
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
