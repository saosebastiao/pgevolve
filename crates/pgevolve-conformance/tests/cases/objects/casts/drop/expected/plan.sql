-- @pgevolve plan id=ba716585d033b862 version=0.4.5 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=drop_cast destructive=false targets=app.celsius,pg_catalog.text
DROP CAST (app.celsius AS pg_catalog.text);
COMMIT;

