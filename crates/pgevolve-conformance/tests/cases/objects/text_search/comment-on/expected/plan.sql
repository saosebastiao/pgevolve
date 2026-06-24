-- @pgevolve plan id=36c5748d0878985a version=0.4.5 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=comment_on_ts_dictionary destructive=false targets=app.en
COMMENT ON TEXT SEARCH DICTIONARY app.en IS 'English snowball stemmer';
-- @pgevolve step=2 kind=comment_on_ts_configuration destructive=false targets=app.cfg
COMMENT ON TEXT SEARCH CONFIGURATION app.cfg IS 'English full-text search configuration';
COMMIT;

