-- @pgevolve plan id=a6813194bfad01cc version=0.4.0 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_collation destructive=false targets=app.ci
CREATE COLLATION app.ci (provider = libc, locale = 'C');
-- @pgevolve step=2 kind=create_table destructive=false targets=app.users
CREATE TABLE app.users (
    id bigint NOT NULL,
    email text COLLATE app.ci NOT NULL,
    CONSTRAINT users_pkey PRIMARY KEY (id)
);
COMMIT;

