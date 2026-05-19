-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE FUNCTION app.get_value() RETURNS integer
    LANGUAGE sql IMMUTABLE
AS $$ SELECT 42 $$;
