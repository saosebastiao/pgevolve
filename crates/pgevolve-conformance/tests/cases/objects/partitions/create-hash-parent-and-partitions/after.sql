-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.events (
    id      bigint NOT NULL,
    payload jsonb  NOT NULL
) PARTITION BY HASH (id);
CREATE TABLE app.events_p0
    PARTITION OF app.events
    FOR VALUES WITH (MODULUS 4, REMAINDER 0);
CREATE TABLE app.events_p1
    PARTITION OF app.events
    FOR VALUES WITH (MODULUS 4, REMAINDER 1);
CREATE TABLE app.events_p2
    PARTITION OF app.events
    FOR VALUES WITH (MODULUS 4, REMAINDER 2);
CREATE TABLE app.events_p3
    PARTITION OF app.events
    FOR VALUES WITH (MODULUS 4, REMAINDER 3);
