-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.events (
  id bigint NOT NULL,
  name text,
  CONSTRAINT events_pkey PRIMARY KEY (id)
);
CREATE FUNCTION app.audit_stamp() RETURNS trigger
    LANGUAGE plpgsql
AS $$
BEGIN
  RETURN NEW;
END
$$;
