-- @pgevolve plan id=da2601d3ec742c06 version=0.4.2 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_table destructive=false targets=app.logs
CREATE TABLE app.logs (
    id bigint NOT NULL,
    logged_at date NOT NULL,
    message text NOT NULL
) PARTITION BY RANGE (logged_at);
-- @pgevolve step=2 kind=create_table destructive=false targets=app.logs_hot
CREATE TABLE app.logs_hot PARTITION OF app.logs FOR VALUES FROM ('2025-01-01') TO ('2026-01-01') TABLESPACE ts_fast;
COMMIT;

