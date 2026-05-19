-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE FUNCTION app.double(x integer) RETURNS integer
    LANGUAGE sql IMMUTABLE STRICT
AS $$ SELECT x + x $$;
