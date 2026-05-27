-- @pgevolve plan id=ecd6e54a14213c1f version=0.3.4 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=add_column destructive=false targets=app.users
ALTER TABLE app.users ADD COLUMN email text;
COMMIT;

