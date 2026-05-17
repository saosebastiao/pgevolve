# failure/lint-at-plan

Lint-at-plan failure fixtures require running `pgevolve plan` as a subprocess
and inspecting exit code and stderr, because the gating logic lives in the CLI
command layer, not in `pgevolve-core`.

This subtree stays empty until T4 follow-up work wires CLI orchestration into
the conformance runner (spawn binary, check exit status, assert stderr substrings).
