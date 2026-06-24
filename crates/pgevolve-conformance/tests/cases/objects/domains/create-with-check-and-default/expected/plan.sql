-- @pgevolve plan id=47d9df7c74a73387 version=0.4.5 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_type destructive=false targets=app.positive_int
CREATE DOMAIN app.positive_int AS integer DEFAULT 1 NOT NULL CONSTRAINT positive CHECK (value > 0);
COMMIT;

