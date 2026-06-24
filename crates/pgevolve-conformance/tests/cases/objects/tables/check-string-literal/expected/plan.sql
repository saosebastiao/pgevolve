-- @pgevolve plan id=9d9b15055349837e version=0.4.5 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=add_constraint_not_valid destructive=false targets=app.t
ALTER TABLE app.t ADD CONSTRAINT t_s_nonempty CHECK (s <> '') NOT VALID;
-- @pgevolve step=2 kind=validate_constraint destructive=false targets=app.t
ALTER TABLE app.t VALIDATE CONSTRAINT t_s_nonempty;
COMMIT;

