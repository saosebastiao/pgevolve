-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE FUNCTION app.now_plus(n integer) RETURNS timestamp
    LANGUAGE sql IMMUTABLE STRICT
AS $$ SELECT '2024-01-01'::timestamp + (n || ' days')::interval $$;
