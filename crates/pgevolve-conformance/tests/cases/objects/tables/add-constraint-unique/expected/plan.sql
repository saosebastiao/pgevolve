-- @pgevolve plan id=9cd7063efaee08e4 version=0.3.6 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=add_constraint destructive=false targets=app.users
ALTER TABLE app.users ADD CONSTRAINT users_email_uq UNIQUE (email);
COMMIT;

