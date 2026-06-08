-- @pgevolve plan id=74c424110da45c5b version=0.4.2 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_aggregate destructive=false targets=app.bad
CREATE AGGREGATE app.bad(integer) (SFUNC = app.ghost_sfunc, STYPE = integer);
COMMIT;

