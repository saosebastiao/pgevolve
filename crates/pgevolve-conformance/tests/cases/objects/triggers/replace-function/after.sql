-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.sessions (
  id         bigint NOT NULL,
  user_id    bigint NOT NULL,
  CONSTRAINT sessions_pkey PRIMARY KEY (id)
);
CREATE FUNCTION app.fn_a() RETURNS trigger
    LANGUAGE plpgsql
AS $$
BEGIN
  RETURN NEW;
END
$$;
CREATE FUNCTION app.fn_b() RETURNS trigger
    LANGUAGE plpgsql
AS $$
BEGIN
  RETURN NEW;
END
$$;
CREATE TRIGGER trg_sessions_hook
  AFTER INSERT ON app.sessions
  FOR EACH ROW
  EXECUTE FUNCTION app.fn_b();
