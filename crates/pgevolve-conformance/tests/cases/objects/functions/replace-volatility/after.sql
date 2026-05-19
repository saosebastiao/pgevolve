-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE FUNCTION app.now_plus(n integer) RETURNS timestamp
    LANGUAGE sql STRICT
AS $$ SELECT now() + (n || ' days')::interval $$;
