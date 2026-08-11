-- @pgevolve plan id=ca949839fe007939 version=0.4.6 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=add_column destructive=false targets=app.items
ALTER TABLE app.items ADD COLUMN doubled integer GENERATED ALWAYS AS (qty * 2) VIRTUAL;
COMMIT;

