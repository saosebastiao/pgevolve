-- @pgevolve plan id=059e677112ac039d version=0.4.6 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_ts_configuration destructive=false targets=app.cfg
CREATE TEXT SEARCH CONFIGURATION app.cfg (PARSER = pg_catalog."default");
COMMIT;

