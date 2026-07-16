# Validation status

## Node application-owned / Windows

Status: **maintained for Windows; deterministic and native gates pass**.

The passing Windows run on 2026-07-17 used:

- AkuSupervisor `0.1.0`, development binary SHA-256
  `351d4fd7d0455344e46f586c82db3b6790bdbfffbe444d4cd35a4416127aa3e9`;
- Node.js `v24.16.0` at the selected NVM executable; and
- fixture contract version `1`, conformance version `0.1.0`, run ID
  `20260716T230951718Z-33932`.

The deterministic application test passed idempotent shutdown, active-request
draining, resource cleanup, and listener release. The native run also proved:

- the independent `/health` endpoint became reachable;
- AkuSupervisor delivered Ctrl+Break and the application observed `SIGBREAK`;
- every required application cleanup event was logged;
- the process exited without forced fallback and left an empty owned tree;
- the listener port was released; and
- an unrelated sentinel process remained running.

The earlier failed run remains useful diagnostic history. Two Windows launch
boundaries had been conflated: Rust's `Command::spawn` discarded the primary
thread handle returned by `CreateProcessW`, forcing the Supervisor to infer a
thread from a process-wide snapshot; and the service inherited the
Supervisor's redirected stdin pipe. The corrected native executable path keeps
the real primary-thread handle, removes exactly the Supervisor-owned suspend
count after Job assignment, captures stdout/stderr through bounded log pipes,
and gives the non-interactive service an inherited `NUL` stdin handle. It does
not enumerate or resume antivirus/EDR-owned threads.

This maintained claim is scoped to the Node application-owned fixture on
Windows. It does not imply Linux or macOS certification and does not turn the
conformance repository into a core build or stable-promotion dependency.

Generated reports remain local under `.artifacts/reports` and are ignored by
Git. This document retains only the reviewed, non-secret conclusion.
