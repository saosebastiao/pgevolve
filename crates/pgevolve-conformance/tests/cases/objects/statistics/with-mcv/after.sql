-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.t (a bigint, b text);
-- Explicit non-default subset: ndistinct + mcv only (no dependencies).
-- The renderer emits the (kinds) clause because it is not the default-all set.
CREATE STATISTICS app.s (ndistinct, mcv) ON a, b FROM app.t;
