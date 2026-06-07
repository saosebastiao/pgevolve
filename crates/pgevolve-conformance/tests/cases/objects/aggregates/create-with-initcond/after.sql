-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE FUNCTION app.sum_sfunc(bigint, integer) RETURNS bigint LANGUAGE plpgsql AS $$ BEGIN RETURN $1 + $2; END $$;
CREATE AGGREGATE app.my_sum(integer) (SFUNC = app.sum_sfunc, STYPE = bigint, INITCOND = '0');
