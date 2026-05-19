-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE FUNCTION app.counts() RETURNS integer
    LANGUAGE sql IMMUTABLE
AS $$ SELECT 1 $$;
