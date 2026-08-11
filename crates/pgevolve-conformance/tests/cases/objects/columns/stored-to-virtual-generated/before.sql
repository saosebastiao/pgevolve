-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.items (
    id      bigint NOT NULL,
    qty     integer NOT NULL,
    doubled integer GENERATED ALWAYS AS (qty * 2) STORED,
    CONSTRAINT items_pkey PRIMARY KEY (id)
);
