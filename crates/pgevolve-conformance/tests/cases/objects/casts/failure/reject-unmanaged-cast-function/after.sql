-- @pgevolve schema=app
CREATE SCHEMA app;
-- Conversion function `app.ghost_cast_fn` is not declared as a managed
-- SQL/plpgsql function in this project, so the
-- cast-references-unmanaged-function lint (Error severity) fires at CLI plan
-- time. The in-process planner still generates the CREATE CAST step; the apply
-- against a real Postgres fails because the cast function does not exist.
CREATE CAST (pg_catalog.int4 AS pg_catalog.text) WITH FUNCTION app.ghost_cast_fn(pg_catalog.int4);
