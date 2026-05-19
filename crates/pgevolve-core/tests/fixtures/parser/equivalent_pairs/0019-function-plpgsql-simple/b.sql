-- @pgevolve schema=app
CREATE FUNCTION greet(name text) RETURNS text
    LANGUAGE plpgsql
    AS $$ BEGIN RETURN 'hello ' || name; END $$;
