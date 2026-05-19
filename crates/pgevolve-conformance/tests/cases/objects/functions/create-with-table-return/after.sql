-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE FUNCTION app.get_summary() RETURNS TABLE(id integer, label text)
    LANGUAGE sql STABLE
AS $$
  SELECT 1, 'first'
  UNION ALL
  SELECT 2, 'second'
$$;
