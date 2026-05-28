-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE COLLATION app.sort (provider = libc, locale = 'C');
