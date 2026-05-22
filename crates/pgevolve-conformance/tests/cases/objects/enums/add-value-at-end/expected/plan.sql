-- @pgevolve plan id=837eb15d72d4b7e4 version=0.3.0 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=alter_type_add_value destructive=false targets=app.status
ALTER TYPE app.status ADD VALUE 'pending' AFTER 'inactive';
COMMIT;

