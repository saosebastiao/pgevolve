-- @pgevolve plan id=b4f34397546b9f72 version=0.4.6 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=comment_on_cast destructive=false targets=app.celsius,pg_catalog.text
COMMENT ON CAST (app.celsius AS pg_catalog.text) IS 'converts celsius domain to text';
COMMIT;

