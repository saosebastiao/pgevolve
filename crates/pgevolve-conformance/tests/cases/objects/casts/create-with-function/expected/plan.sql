-- @pgevolve plan id=522a14827c8713a5 version=0.4.3 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_cast destructive=false targets=app.celsius,pg_catalog.text
CREATE CAST (app.celsius AS pg_catalog.text) WITH FUNCTION app.celsius_to_text(app.celsius);
COMMIT;

