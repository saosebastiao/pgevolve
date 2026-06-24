-- @pgevolve plan id=ee5d196f427162ce version=0.4.6 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_table destructive=false targets=app.clone
CREATE TABLE app.clone (
    x integer,
    a integer,
    b integer,
    y text
);
COMMIT;

