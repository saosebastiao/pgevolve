-- @pgevolve plan id=66b4e0a37d15d348 version=0.3.1 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=alter_domain_add_constraint destructive=false targets=app.score
ALTER DOMAIN app.score ADD CONSTRAINT valid_score CHECK (value >= 0 and value <= 100);
COMMIT;

