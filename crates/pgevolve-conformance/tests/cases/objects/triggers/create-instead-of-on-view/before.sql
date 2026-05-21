-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.users (
  id    bigint NOT NULL,
  name  text   NOT NULL,
  CONSTRAINT users_pkey PRIMARY KEY (id)
);
CREATE VIEW app.users_view AS
  SELECT id, name FROM app.users;
CREATE FUNCTION app.upsert_user() RETURNS trigger
    LANGUAGE plpgsql
AS $$
BEGIN
  INSERT INTO app.users (id, name) VALUES (NEW.id, NEW.name)
    ON CONFLICT (id) DO UPDATE SET name = EXCLUDED.name;
  RETURN NEW;
END
$$;
