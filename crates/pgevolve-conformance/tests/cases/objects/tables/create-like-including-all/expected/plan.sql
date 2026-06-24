-- @pgevolve plan id=e6dad57c44e933db version=0.4.5 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_table destructive=false targets=app.clone
CREATE TABLE app.clone (
    id bigint NOT NULL DEFAULT 1,
    email text,
    data text,
    CONSTRAINT base_email_chk CHECK (email <> ''),
    CONSTRAINT clone_pkey PRIMARY KEY (id),
    CONSTRAINT clone_email_key UNIQUE (email)
);
-- @pgevolve step=2 kind=create_index destructive=false targets=app.clone_data_idx,app.clone
CREATE INDEX clone_data_idx ON app.clone USING btree (data);
COMMIT;

