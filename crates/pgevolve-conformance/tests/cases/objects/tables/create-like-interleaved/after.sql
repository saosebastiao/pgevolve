-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.base (a int, b int);
CREATE TABLE app.clone (x int, LIKE app.base, y text);
