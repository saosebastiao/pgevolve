-- @pgevolve plan id=a2be089d88b09ffe version=0.1.0-dev ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=2

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=drop_function destructive=true intent_id=1 targets=app.counts
DROP FUNCTION app.counts();
-- @pgevolve step=2 kind=create_or_replace_function destructive=true intent_id=2 targets=app.counts
CREATE OR REPLACE FUNCTION app.counts()
    RETURNS SETOF integer
    LANGUAGE sql IMMUTABLE
AS $pgevolve$SELECT 1 UNION ALL SELECT 2$pgevolve$;
COMMIT;

