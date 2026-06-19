-- @pgevolve plan id=4b97c78e2ff6be4f version=0.4.4 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=set_column_storage destructive=false targets=app.docs
ALTER TABLE app.docs ALTER COLUMN body SET STORAGE PLAIN;
COMMIT;

