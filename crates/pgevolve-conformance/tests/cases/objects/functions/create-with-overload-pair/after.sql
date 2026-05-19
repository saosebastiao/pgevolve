-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE FUNCTION app.add(a integer, b integer) RETURNS integer
    LANGUAGE sql IMMUTABLE STRICT
AS $$ SELECT a + b $$;
CREATE FUNCTION app.add(a text, b text) RETURNS text
    LANGUAGE sql IMMUTABLE STRICT
AS $$ SELECT a || b $$;
