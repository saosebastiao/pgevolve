-- @pgevolve plan id=bb9bb7c58aa8cb95 version=0.3.8 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=set_table_row_security destructive=false targets=app.docs
ALTER TABLE app.docs ENABLE ROW LEVEL SECURITY;
COMMIT;

