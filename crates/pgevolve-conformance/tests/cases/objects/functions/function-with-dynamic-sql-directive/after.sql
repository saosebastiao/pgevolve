-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.reports (
  id bigint NOT NULL,
  name text,
  CONSTRAINT reports_pkey PRIMARY KEY (id)
);
CREATE FUNCTION app.run_report() RETURNS void
    LANGUAGE plpgsql
AS $$
DECLARE
  -- @pgevolve dep: app.reports
  v_sql text;
BEGIN
  v_sql := 'SELECT count(*) FROM app.reports';
  EXECUTE v_sql;
END
$$;
