-- @pgevolve plan id=55d91da27f757b9b version=0.3.7 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_collation destructive=false targets=app.und_co
CREATE COLLATION app.und_co (provider = icu, locale = 'und');
COMMIT;

