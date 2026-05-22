-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.items (
    id       bigint NOT NULL,
    priority integer DEFAULT 0,
    CONSTRAINT items_pkey PRIMARY KEY (id)
);
