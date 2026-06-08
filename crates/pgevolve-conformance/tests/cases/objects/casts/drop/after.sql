-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE DOMAIN app.celsius AS numeric;
CREATE FUNCTION app.celsius_to_text(app.celsius) RETURNS text LANGUAGE plpgsql AS $$ BEGIN RETURN $1::text || 'C'; END $$;
