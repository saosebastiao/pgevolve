-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE FUNCTION app.base_value() RETURNS integer
    LANGUAGE sql IMMUTABLE
AS $$ SELECT 10 $$;
CREATE FUNCTION app.doubled_value() RETURNS integer
    LANGUAGE plpgsql IMMUTABLE
AS $$
DECLARE
  -- @pgevolve dep: app.base_value
  v integer;
BEGIN
  SELECT app.base_value() INTO v;
  RETURN v * 2;
END
$$;
