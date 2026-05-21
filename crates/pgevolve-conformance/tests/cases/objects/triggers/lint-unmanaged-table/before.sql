-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE FUNCTION app.on_ghost_insert() RETURNS trigger
    LANGUAGE plpgsql
AS $$
BEGIN
  RETURN NEW;
END
$$;
