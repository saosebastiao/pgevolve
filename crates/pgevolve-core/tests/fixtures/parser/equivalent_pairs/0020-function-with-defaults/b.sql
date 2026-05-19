-- @pgevolve schema=app
CREATE FUNCTION greet(name text DEFAULT 'world') RETURNS text
    LANGUAGE sql
    AS $$ SELECT 'hello ' || name $$;
