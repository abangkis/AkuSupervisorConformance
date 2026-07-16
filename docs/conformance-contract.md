# Conformance contract

Contract version `1` defines the current fixture manifest and report shapes.
The JSON schemas in `schemas` are the canonical machine-readable contracts.

## Required native checks

Every application-owned runtime fixture must prove:

1. its deterministic shutdown test passed;
2. AkuSupervisor started the direct runtime executable and reached process
   health;
3. the runner independently reached the fixture's declared application health
   endpoint;
4. the native stop reported `gracefulSignalSent: true`;
5. the same stop reported `forced: false` and `ownedPidsAfter: []`;
6. application logs recorded `shutdown_started` and `shutdown_completed` for
   the expected native signal;
7. the lifecycle journal contains matching shutdown evidence;
8. the declared listener port was released; and
9. a process outside the Supervisor ownership boundary remained alive.

The Node fixture additionally records `resource_cleanup_completed`. This makes
`forced: false` insufficient on its own: the application must prove that its
handler and declared cleanup path ran.

Process health inside AkuSupervisor and application readiness in the runner are
separate on purpose. This suite certifies cooperative shutdown, not the
Supervisor's HTTP probe implementation; a later health-adapter suite can cover
that contract independently without changing the runtime recipe result.

## Result semantics

- `passed`: every required check passed.
- `failed`: execution occurred and any required check failed.
- `skipped`: the requested OS or runtime prerequisite was unavailable.

Reports are observations, not executable configuration. They must not include
control tokens or application secrets. Paths are allowed because they bind a
local run to its artifacts; published evidence should be reviewed before
sharing outside its originating environment.

## Versioning

- Increment the fixture `contractVersion` when observable behavior or required
  evidence changes.
- Increment the report `schemaVersion` only for a breaking report shape.
- Record both project versions and the AkuSupervisor binary SHA-256 in every
  native report.
- A compatibility claim identifies an OS, architecture, runtime version,
  fixture contract version, AkuSupervisor version/hash, and result timestamp.
