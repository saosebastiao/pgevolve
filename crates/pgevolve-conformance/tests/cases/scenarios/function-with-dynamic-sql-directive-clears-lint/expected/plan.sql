-- @pgevolve plan id=5f347f39495a90ec version=0.3.0 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_table destructive=false targets=app.audit_log
CREATE TABLE app.audit_log (
    id bigint NOT NULL,
    msg text,
    CONSTRAINT audit_log_pkey PRIMARY KEY (id)
);
-- @pgevolve step=2 kind=create_or_replace_function destructive=false targets=app.log_event
CREATE OR REPLACE FUNCTION app.log_event(p_msg text)
    RETURNS void
    LANGUAGE plpgsql
AS $pgevolve$DECLARE -- @pgevolve dep: app.audit_log
v_sql text; BEGIN v_sql := format('INSERT INTO app.audit_log (id, msg) VALUES (%s, %L)', 1, p_msg); EXECUTE v_sql; END$pgevolve$;
COMMIT;

