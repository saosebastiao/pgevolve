-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE PROCEDURE app.cleanup()
    LANGUAGE plpgsql
AS $$
BEGIN
  NULL;
END
$$;
