-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE PROCEDURE app.do_nothing()
    LANGUAGE plpgsql
AS $$
BEGIN
  NULL;
END
$$;
