-- @pgevolve plan id=6ac651f92b1ac445 version=0.1.0-dev ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=detach_partition destructive=false targets=app.sales_window
ALTER TABLE app.sales DETACH PARTITION app.sales_window;
-- @pgevolve step=2 kind=attach_partition destructive=false targets=app.sales_window
ALTER TABLE app.sales ATTACH PARTITION app.sales_window FOR VALUES FROM ('2024-01-01') TO ('2026-01-01');
COMMIT;

