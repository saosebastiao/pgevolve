-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE COLLATION app.c_libc (provider = libc, locale = 'C');
COMMENT ON COLLATION app.c_libc IS 'pinned for binary sorting';
