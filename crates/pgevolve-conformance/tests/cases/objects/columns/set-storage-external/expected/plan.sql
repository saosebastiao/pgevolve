-- @pgevolve plan id=af61ceb1e50e37ec version=0.3.7 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=set_column_storage destructive=false targets=app.docs
ALTER TABLE app.docs ALTER COLUMN body SET STORAGE EXTERNAL;
COMMIT;

