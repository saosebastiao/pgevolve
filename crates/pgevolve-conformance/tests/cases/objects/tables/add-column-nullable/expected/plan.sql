-- @pgevolve plan id=2ffd1e9e05c6443b version=0.4.6 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=add_column destructive=false targets=app.users
ALTER TABLE app.users ADD COLUMN email text;
COMMIT;

