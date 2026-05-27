-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.t (a bigint, b bigint, c bigint);
CREATE STATISTICS app.s (ndistinct) ON a, b FROM app.t;
