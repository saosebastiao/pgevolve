CREATE FUNCTION app.greet(name text DEFAULT 'world') RETURNS text
    LANGUAGE sql
    AS $$ SELECT 'hello ' || name $$;
