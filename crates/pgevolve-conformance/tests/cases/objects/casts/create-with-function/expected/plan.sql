-- @pgevolve plan id=083e4e0db5c32c3f version=0.4.5 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_cast destructive=false targets=app.celsius,pg_catalog.text
CREATE CAST (app.celsius AS pg_catalog.text) WITH FUNCTION app.celsius_to_text(app.celsius);
COMMIT;

