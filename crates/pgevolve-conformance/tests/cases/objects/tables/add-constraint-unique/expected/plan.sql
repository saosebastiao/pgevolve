-- @pgevolve plan id=63164d10c68c7e9e version=0.4.5 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=add_constraint destructive=false targets=app.users
ALTER TABLE app.users ADD CONSTRAINT users_email_uq UNIQUE (email);
COMMIT;

