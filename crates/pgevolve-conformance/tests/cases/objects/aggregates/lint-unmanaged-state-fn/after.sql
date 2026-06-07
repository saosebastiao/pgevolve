-- @pgevolve schema=app
CREATE SCHEMA app;
-- SFUNC `app.ghost_sfunc` is not declared as a managed SQL/plpgsql function in
-- this project, so the aggregate-references-unmanaged-function lint (Error
-- severity) fires at CLI plan time. The in-process planner still generates the
-- CREATE AGGREGATE step; the apply against a real Postgres fails because the
-- state function does not exist.
CREATE AGGREGATE app.bad(integer) (SFUNC = app.ghost_sfunc, STYPE = integer);
