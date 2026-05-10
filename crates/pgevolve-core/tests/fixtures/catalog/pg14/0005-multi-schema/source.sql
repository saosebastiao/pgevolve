CREATE SCHEMA app;
CREATE SCHEMA billing;

CREATE TABLE app.users (
    id    bigint PRIMARY KEY,
    email text NOT NULL
);

CREATE TABLE billing.invoices (
    id      bigint PRIMARY KEY,
    user_id bigint NOT NULL,
    cents   bigint NOT NULL
);
