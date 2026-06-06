-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.t (
    id bigint NOT NULL,
    CONSTRAINT t_pkey PRIMARY KEY (id)
) USING heap;
