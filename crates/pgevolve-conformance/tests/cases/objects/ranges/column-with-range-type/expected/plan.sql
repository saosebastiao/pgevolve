-- @pgevolve plan id=4f29b37bca04e00c version=0.3.9 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_type destructive=false targets=app.int_window
CREATE TYPE app.int_window AS RANGE (subtype = pg_catalog.int4);
-- @pgevolve step=2 kind=create_table destructive=false targets=app.reservations
CREATE TABLE app.reservations (
    id bigint NOT NULL,
    span app.int_window NOT NULL,
    CONSTRAINT reservations_pkey PRIMARY KEY (id)
);
COMMIT;

