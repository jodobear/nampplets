# Verification artifacts

CI generates two deterministic architecture artifacts:

- `nmp-architecture-scan.json` is bounded static triage from the
  repository-owned NMP architecture scanner. CI includes at most 200 findings
  and records both the total and whether the artifact was truncated.
- `d0-d10-report.json` combines that raw output with the reviewed evidence
  inventory in `d0-d10-evidence.json`.

The report deliberately makes no automatic compliance claim. Scanner warnings
remain in the artifact for review, and scanner errors fail CI. The checked-in
evidence dispositions distinguish implemented design evidence, behavior
delegated to the NMP facade, and doctrine that the current M0 surface does not
exercise.
