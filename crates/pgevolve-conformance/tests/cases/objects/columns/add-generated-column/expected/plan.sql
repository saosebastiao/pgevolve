-- @pgevolve plan id=efe4b7575c48b5e3 version=0.3.3 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=add_column destructive=false targets=app.measurements
ALTER TABLE app.measurements ADD COLUMN area numeric GENERATED ALWAYS AS (width * height) STORED;
COMMIT;

