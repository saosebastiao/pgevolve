-- @pgevolve plan id=521bac877a4a42f4 version=0.3.3 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=alter_type_add_value destructive=false targets=app.priority
ALTER TYPE app.priority ADD VALUE 'low' BEFORE 'medium';
COMMIT;

