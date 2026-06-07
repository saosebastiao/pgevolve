-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE FUNCTION app.sum_sfunc(bigint, integer) RETURNS bigint LANGUAGE plpgsql AS $$ BEGIN RETURN $1 + $2; END $$;
