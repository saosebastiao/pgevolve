-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TEXT SEARCH DICTIONARY app.en (TEMPLATE = pg_catalog.snowball, language = 'english');
CREATE TEXT SEARCH CONFIGURATION app.cfg (PARSER = pg_catalog."default");
ALTER TEXT SEARCH CONFIGURATION app.cfg ADD MAPPING FOR word WITH app.en;
