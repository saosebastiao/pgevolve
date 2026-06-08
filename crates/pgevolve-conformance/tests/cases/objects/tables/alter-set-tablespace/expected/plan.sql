-- @pgevolve plan id=7d6d2774124749df version=0.4.2 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=1

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=alter_table_set_tablespace destructive=true intent_id=1 targets=app.orders
ALTER TABLE app.orders SET TABLESPACE ts_fast;
COMMIT;

