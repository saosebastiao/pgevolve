-- @pgevolve plan id=fb0aa18d37ce9915 version=0.4.5 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=add_column destructive=false targets=app.measurements
ALTER TABLE app.measurements ADD COLUMN area numeric GENERATED ALWAYS AS (width * height) STORED;
COMMIT;

