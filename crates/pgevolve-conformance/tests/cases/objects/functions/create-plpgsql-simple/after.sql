-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE FUNCTION app.triple(n integer) RETURNS integer
    LANGUAGE plpgsql IMMUTABLE STRICT
AS $$
BEGIN
  RETURN n * 3;
END
$$;
