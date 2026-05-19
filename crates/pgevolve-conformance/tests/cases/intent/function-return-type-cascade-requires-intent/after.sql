-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE FUNCTION app.get_value() RETURNS SETOF integer
    LANGUAGE sql IMMUTABLE
AS $$ SELECT 42 UNION ALL SELECT 99 $$;
