-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.logs (
    id      bigint NOT NULL,
    source  text   NOT NULL,
    message text   NOT NULL
) PARTITION BY LIST (source);
CREATE TABLE app.logs_app
    PARTITION OF app.logs
    FOR VALUES IN ('app');
CREATE TABLE app.logs_other
    PARTITION OF app.logs
    DEFAULT;
