-- @pgevolve plan id=56a86692750d3d39 version=0.3.5 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=alter_type_rename_value destructive=false targets=app.status
ALTER TYPE app.status RENAME VALUE 'inactive' TO 'archived';
COMMIT;

