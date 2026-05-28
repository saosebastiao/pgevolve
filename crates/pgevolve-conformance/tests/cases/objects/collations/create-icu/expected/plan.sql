-- @pgevolve plan id=98613bb0bc440b33 version=0.3.8 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_collation destructive=false targets=app.und_co
CREATE COLLATION app.und_co (provider = icu, locale = 'und');
COMMIT;

