-- @pgevolve plan id=1a4f9393e60e7bd0 version=0.3.3 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=set_column_compression destructive=false targets=app.docs
ALTER TABLE app.docs ALTER COLUMN body SET COMPRESSION lz4;
COMMIT;

