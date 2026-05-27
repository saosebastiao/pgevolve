-- @pgevolve plan id=86f3f3993290bd1b version=0.3.5 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=set_table_row_security destructive=false targets=app.docs
ALTER TABLE app.docs ENABLE ROW LEVEL SECURITY;
COMMIT;

