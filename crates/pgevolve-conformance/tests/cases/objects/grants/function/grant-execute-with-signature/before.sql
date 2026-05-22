-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE FUNCTION app.foo(int) RETURNS int LANGUAGE sql AS 'SELECT $1';
GRANT EXECUTE ON FUNCTION app.foo(int) TO PUBLIC;
