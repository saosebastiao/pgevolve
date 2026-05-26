-- @pgevolve plan id=caada4fac8de5aac version=0.3.3 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=alter_type_add_value destructive=false targets=app.status
ALTER TYPE app.status ADD VALUE 'pending' AFTER 'inactive';
COMMIT;

