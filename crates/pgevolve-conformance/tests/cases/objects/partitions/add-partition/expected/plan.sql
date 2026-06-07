-- @pgevolve plan id=a532d36eb6a2b119 version=0.4.1 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_table destructive=false targets=app.invoices_2025
CREATE TABLE app.invoices_2025 PARTITION OF app.invoices FOR VALUES FROM ('2025-01-01') TO ('2026-01-01');
COMMIT;

