-- @pgevolve plan id=cd3a13c49c5fa07f version=0.4.4 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=set_table_row_security destructive=false targets=app.docs
ALTER TABLE app.docs ENABLE ROW LEVEL SECURITY;
-- @pgevolve step=2 kind=set_table_force_row_security destructive=false targets=app.docs
ALTER TABLE app.docs FORCE ROW LEVEL SECURITY;
COMMIT;

