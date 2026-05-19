-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.audit_log (
  id bigint NOT NULL,
  msg text,
  CONSTRAINT audit_log_pkey PRIMARY KEY (id)
);
CREATE FUNCTION app.log_event(p_msg text) RETURNS void
    LANGUAGE plpgsql
AS $$
DECLARE
  -- @pgevolve dep: app.audit_log
  v_sql text;
BEGIN
  v_sql := format('INSERT INTO app.audit_log (id, msg) VALUES (%s, %L)', 1, p_msg);
  EXECUTE v_sql;
END
$$;
