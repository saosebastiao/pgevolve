-- @pgevolve plan id=8835c69e827fb05f version=0.3.9 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=1

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=replace_publication destructive=true intent_id=1 targets=
DROP PUBLICATION main;
CREATE PUBLICATION main FOR TABLE app.x;
COMMIT;

