# failure/cycle

Body-derived dependency cycles (Decision 8 — PlanError::BodyCycle) only
manifest when v0.2 body-bearing objects (views, MVs, functions) exist.
This subtree stays empty until the v0.2 view/function sub-spec lands and
the runner can construct a cycle-producing source IR.
