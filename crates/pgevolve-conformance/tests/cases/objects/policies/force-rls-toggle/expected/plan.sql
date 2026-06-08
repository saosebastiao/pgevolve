-- @pgevolve plan id=9a1be63f8b9dc41c version=0.4.2 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=set_table_force_row_security destructive=false targets=app.docs
ALTER TABLE app.docs FORCE ROW LEVEL SECURITY;
COMMIT;

