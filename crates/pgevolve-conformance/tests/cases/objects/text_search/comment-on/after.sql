-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TEXT SEARCH DICTIONARY app.en (TEMPLATE = pg_catalog.snowball, language = 'english');
COMMENT ON TEXT SEARCH DICTIONARY app.en IS 'English snowball stemmer';
CREATE TEXT SEARCH CONFIGURATION app.cfg (PARSER = pg_catalog."default");
COMMENT ON TEXT SEARCH CONFIGURATION app.cfg IS 'English full-text search configuration';
