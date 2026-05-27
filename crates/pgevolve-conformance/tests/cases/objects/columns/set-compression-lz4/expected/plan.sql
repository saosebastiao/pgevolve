-- @pgevolve plan id=8a5d9581fa550a5e version=0.3.4 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=set_column_compression destructive=false targets=app.docs
ALTER TABLE app.docs ALTER COLUMN body SET COMPRESSION lz4;
COMMIT;

