-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE FUNCTION app.counts() RETURNS SETOF integer
    LANGUAGE sql IMMUTABLE
AS $$ SELECT 1 UNION ALL SELECT 2 $$;
