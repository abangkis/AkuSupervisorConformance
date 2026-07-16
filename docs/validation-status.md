# Validation status

## Node application-owned / Windows

Status: **candidate; deterministic gate passes, native gate correctly fails**.

The initial Windows run on 2026-07-17 used:

- AkuSupervisor `0.1.0`, development binary SHA-256
  `0fdae35de0e4a727cc4ede33ac448cb78f78296226b308bc15c84abf16b94bca`;
- Node.js `v24.16.0` at the selected NVM executable; and
- fixture contract version `1`.

The deterministic application test passed idempotent shutdown, active-request
draining, resource cleanup, and listener release. The native run then exposed a
real Windows startup defect before certification:

- AkuSupervisor owned one Node PID and process health was `healthy`;
- the Node entrypoint emitted no `server_ready` event and opened no listener;
- Ctrl+Break was reported as sent and the owned tree became empty without
  forced fallback; but
- no application shutdown event or observed `SIGBREAK` existed.

Therefore `forced: false` was correctly rejected as sufficient recipe evidence.
The current Windows process adapter starts a child with `CREATE_SUSPENDED`,
enumerates threads by PID, resumes the first matching thread, and accepts a
`ResumeThread` return value of zero. Zero means that selected thread was not
suspended. With an EDR/antivirus-injected thread present, this can leave the
actual primary thread suspended while reporting a successful spawn. The run
behavior and code path are consistent with that race; the core fix and a
passing rerun remain required before changing this tuple to `maintained`.

Generated reports remain local under `.artifacts/reports` and are ignored by
Git. This document retains only the reviewed, non-secret conclusion.

