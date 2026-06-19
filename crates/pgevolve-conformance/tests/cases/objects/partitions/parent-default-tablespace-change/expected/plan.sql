-- @pgevolve plan id=3a0076906b916662 version=0.4.4 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=alter_table_set_tablespace destructive=false targets=app.metrics
ALTER TABLE app.metrics SET TABLESPACE ts_fast;
COMMIT;

