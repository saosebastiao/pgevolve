-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE FUNCTION app.float8_diff(double precision, double precision) RETURNS double precision
    LANGUAGE sql IMMUTABLE STRICT
    AS $$ SELECT $1 - $2 $$;
CREATE TYPE app.float8_range AS RANGE (
    subtype = float8,
    subtype_diff = app.float8_diff
);
