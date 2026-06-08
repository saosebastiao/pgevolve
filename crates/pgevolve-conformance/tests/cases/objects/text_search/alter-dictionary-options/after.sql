-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TEXT SEARCH DICTIONARY app.en (TEMPLATE = pg_catalog.snowball, language = 'dutch');
