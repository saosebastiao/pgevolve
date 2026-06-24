-- @pgevolve plan id=c7ba8c075819655b version=0.4.6 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_table destructive=false targets=app.c
CREATE TABLE app.c (
    a integer,
    mid integer,
    b integer
);
COMMIT;

