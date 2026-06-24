-- @pgevolve plan id=8a38e8d9f26d1844 version=0.4.5 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_collation destructive=false targets=app.ci
CREATE COLLATION app.ci (provider = icu, locale = 'und', deterministic = false);
COMMIT;

