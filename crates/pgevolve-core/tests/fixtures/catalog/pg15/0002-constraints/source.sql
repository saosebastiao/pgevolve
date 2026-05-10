CREATE SCHEMA app;

CREATE TABLE app.orgs (
    id   bigint PRIMARY KEY,
    name text NOT NULL
);

CREATE TABLE app.users (
    id        bigint PRIMARY KEY,
    org_id    bigint NOT NULL REFERENCES app.orgs (id) ON DELETE CASCADE,
    email     text NOT NULL,
    age       integer CHECK (age >= 0),
    UNIQUE (email)
);
