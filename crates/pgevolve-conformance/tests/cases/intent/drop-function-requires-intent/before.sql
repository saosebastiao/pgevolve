-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE FUNCTION app.helper(n integer) RETURNS integer
    LANGUAGE sql IMMUTABLE STRICT
AS $$ SELECT n + 1 $$;
