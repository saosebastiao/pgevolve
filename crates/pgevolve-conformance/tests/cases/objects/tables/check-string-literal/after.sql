-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.t (
    s text,
    CONSTRAINT t_s_nonempty CHECK (s <> '')
);
