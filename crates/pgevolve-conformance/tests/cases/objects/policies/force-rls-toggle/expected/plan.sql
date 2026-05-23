-- @pgevolve plan id=1965a7051e757d20 version=0.3.2 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=set_table_force_row_security destructive=false targets=app.docs
ALTER TABLE app.docs FORCE ROW LEVEL SECURITY;
COMMIT;

