-- @pgevolve plan id=8a24aa06d0d27592 version=0.4.4 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=alter_ts_dictionary destructive=false targets=app.en
ALTER TEXT SEARCH DICTIONARY app.en (language = 'dutch');
COMMIT;

