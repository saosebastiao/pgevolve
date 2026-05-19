-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE PROCEDURE app.greet()
    LANGUAGE plpgsql
AS $$
BEGIN
  RAISE NOTICE 'Hello';
END
$$;
COMMENT ON PROCEDURE app.greet IS 'Prints a greeting notice';
