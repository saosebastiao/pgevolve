-- @pgevolve plan id=28e2887ea8ff4f77 version=0.4.6 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=drop_ts_config_mapping destructive=false targets=app.cfg
ALTER TEXT SEARCH CONFIGURATION app.cfg DROP MAPPING IF EXISTS FOR asciiword;
COMMIT;

