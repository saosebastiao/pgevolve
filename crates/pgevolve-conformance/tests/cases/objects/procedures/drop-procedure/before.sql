-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE PROCEDURE app.old_proc()
    LANGUAGE plpgsql
AS $$
BEGIN
  NULL;
END
$$;
