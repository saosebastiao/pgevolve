-- @pgevolve plan id=9fb91f97af9a456c version=0.4.0 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_collation destructive=false targets=app.ci
CREATE COLLATION app.ci (provider = icu, locale = 'und', deterministic = false);
COMMIT;

