-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TYPE app.textpat_range AS RANGE (
    subtype = text,
    subtype_opclass = pg_catalog.text_pattern_ops
);
