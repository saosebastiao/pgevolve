-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE COLLATION app.c_libc (provider = libc, locale = 'C');
