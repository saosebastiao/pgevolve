-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE PROCEDURE app.greet()
    LANGUAGE plpgsql
AS $$
BEGIN
  RAISE NOTICE 'Hello, world!';
END
$$;
