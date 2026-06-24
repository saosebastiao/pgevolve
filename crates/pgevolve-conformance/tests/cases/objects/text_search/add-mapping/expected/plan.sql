-- @pgevolve plan id=911d5a61aaa9f9e5 version=0.4.5 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_ts_dictionary destructive=false targets=app.en
CREATE TEXT SEARCH DICTIONARY app.en (TEMPLATE = pg_catalog.snowball, language = 'english');
-- @pgevolve step=2 kind=create_ts_configuration destructive=false targets=app.cfg
CREATE TEXT SEARCH CONFIGURATION app.cfg (PARSER = pg_catalog."default");
-- @pgevolve step=3 kind=add_ts_config_mapping destructive=false targets=app.cfg
ALTER TEXT SEARCH CONFIGURATION app.cfg ADD MAPPING FOR asciiword WITH app.en;
-- @pgevolve step=4 kind=add_ts_config_mapping destructive=false targets=app.cfg
ALTER TEXT SEARCH CONFIGURATION app.cfg ADD MAPPING FOR word WITH app.en;
COMMIT;

