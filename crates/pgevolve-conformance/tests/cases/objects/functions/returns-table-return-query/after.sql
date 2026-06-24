-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE FUNCTION app.get_pairs() RETURNS TABLE(a integer, b text)
    LANGUAGE plpgsql STABLE
AS $$
BEGIN
  RETURN QUERY SELECT 1, 'hello';
END
$$;
