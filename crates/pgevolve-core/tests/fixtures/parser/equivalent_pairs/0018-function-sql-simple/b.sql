-- @pgevolve schema=app
CREATE FUNCTION double(x integer) RETURNS integer
    LANGUAGE sql IMMUTABLE STRICT
    AS $$ SELECT x * 2 $$;
