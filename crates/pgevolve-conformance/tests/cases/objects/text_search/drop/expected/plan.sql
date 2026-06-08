-- @pgevolve plan id=b1fad8bb4406e7aa version=0.4.2 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=drop_ts_configuration destructive=false targets=app.cfg
DROP TEXT SEARCH CONFIGURATION app.cfg;
-- @pgevolve step=2 kind=drop_ts_dictionary destructive=false targets=app.en
DROP TEXT SEARCH DICTIONARY app.en;
COMMIT;

