-- @pgevolve plan id=76f53f85b4b43194 version=0.4.2 ruleset=1
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

