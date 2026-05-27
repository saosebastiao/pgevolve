-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.t (a bigint, b bigint, c bigint);
-- Adding column c to the statistic requires DROP + CREATE (ReplaceStatistic).
CREATE STATISTICS app.s (ndistinct) ON a, b, c FROM app.t;
