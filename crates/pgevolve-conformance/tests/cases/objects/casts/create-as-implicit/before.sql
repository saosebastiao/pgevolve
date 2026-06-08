-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE DOMAIN app.score AS integer;
CREATE FUNCTION app.score_to_bigint(app.score) RETURNS bigint LANGUAGE plpgsql AS $$ BEGIN RETURN $1::bigint; END $$;
