-- @pgevolve plan id=dd9908de9e663e3c version=0.4.3 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=drop_cast destructive=false targets=app.celsius,pg_catalog.text
DROP CAST (app.celsius AS pg_catalog.text);
COMMIT;

