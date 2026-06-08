-- @pgevolve plan id=ab757c5373ae9d25 version=0.4.3 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=alter_ts_config_mapping destructive=false targets=app.cfg
ALTER TEXT SEARCH CONFIGURATION app.cfg ALTER MAPPING FOR asciiword WITH app.nl;
-- @pgevolve step=2 kind=alter_ts_config_mapping destructive=false targets=app.cfg
ALTER TEXT SEARCH CONFIGURATION app.cfg ALTER MAPPING FOR word WITH app.nl;
COMMIT;

