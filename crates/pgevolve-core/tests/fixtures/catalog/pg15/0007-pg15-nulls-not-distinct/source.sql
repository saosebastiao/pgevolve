CREATE SCHEMA app;

CREATE TABLE app.t (
    a integer
);

CREATE UNIQUE INDEX t_a_uniq ON app.t (a) NULLS NOT DISTINCT;
