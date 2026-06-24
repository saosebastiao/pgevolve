-- @pgevolve plan id=b0abb823db5f1245 version=0.4.5 ruleset=1
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

