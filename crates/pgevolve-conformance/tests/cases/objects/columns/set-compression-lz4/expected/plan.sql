-- @pgevolve plan id=3a87d8022d7d4553 version=0.4.4 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=set_column_compression destructive=false targets=app.docs
ALTER TABLE app.docs ALTER COLUMN body SET COMPRESSION lz4;
COMMIT;

