CREATE FUNCTION app.greet(name text) RETURNS text
    LANGUAGE plpgsql
    AS $$ BEGIN RETURN 'hello ' || name; END $$;
