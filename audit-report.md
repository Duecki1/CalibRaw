# CalibRaw code audit report

Read-only audit. Entries are appended in discovery order. Known findings from
`finding.md` and `docs/rework/AUDIT.md` are not repeated.

### [UI-031] GPU budget exhaustion is detected by substring-matching a formatted error
- category: bad-code
- severity: medium
- confidence: high
- where: crates/calibraw-ui/src/app/ai/source.rs:143, crates/calibraw-ui/src/app/processing_export/preview/rebuild.rs:314
- evidence: Err(error) if error.to_string().contains("GPU pipelines already reserve") => {
              .is_some_and(|error| error.to_string().contains("GPU pipelines already reserve"));
- why it is a problem: the "pipeline does not fit the GPU budget" condition is a distinct, recoverable outcome produced in `calibraw-gpu/src/pipeline/gpu/resources.rs:139` as an `anyhow!` string, so any wording or formatting change silently turns both fallback paths into hard failures.
- fix: return a typed/classifiable error (or a predicate or error kind) from `GpuBudgetReservation::acquire` and match on it in both places.
- effort: M
