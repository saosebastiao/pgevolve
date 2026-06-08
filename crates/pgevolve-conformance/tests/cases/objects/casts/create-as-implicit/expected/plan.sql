-- @pgevolve plan id=be9d41c0b7d586b1 version=0.4.2 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_cast destructive=false targets=app.score,pg_catalog.int8
CREATE CAST (app.score AS pg_catalog.int8) WITH FUNCTION app.score_to_bigint(app.score) AS IMPLICIT;
COMMIT;

