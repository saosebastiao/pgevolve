-- @pgevolve plan id=dabfa819983e241c version=0.4.2 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_ts_dictionary destructive=false targets=app.en
CREATE TEXT SEARCH DICTIONARY app.en (TEMPLATE = pg_catalog.snowball, language = 'english');
COMMIT;

