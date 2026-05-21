-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.events (
  id      bigint NOT NULL,
  payload text,
  CONSTRAINT events_pkey PRIMARY KEY (id)
);
CREATE FUNCTION app.log_statement() RETURNS trigger
    LANGUAGE plpgsql
AS $$
BEGIN
  RETURN NULL;
END
$$;
CREATE TRIGGER trg_events_log
  AFTER UPDATE ON app.events
  FOR EACH STATEMENT
  EXECUTE FUNCTION app.log_statement();
