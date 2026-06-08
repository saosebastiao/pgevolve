-- @pgevolve plan id=9056b420e7c72b97 version=0.4.1 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_cast destructive=false targets=app.score,pg_catalog.int8
CREATE CAST (app.score AS pg_catalog.int8) WITH FUNCTION app.score_to_bigint(app.score) AS IMPLICIT;
COMMIT;

