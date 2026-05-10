CREATE SCHEMA app;
COMMENT ON SCHEMA app IS 'Application schema';

CREATE TABLE app.users (
    id    bigint PRIMARY KEY,
    email text NOT NULL
);
COMMENT ON TABLE app.users IS 'Application users';
COMMENT ON COLUMN app.users.email IS 'User contact email';

CREATE INDEX users_email_idx ON app.users (email);
COMMENT ON INDEX app.users_email_idx IS 'Lookup users by email';

CREATE SEQUENCE app.id_seq;
COMMENT ON SEQUENCE app.id_seq IS 'Independent ID sequence';

ALTER TABLE app.users ADD CONSTRAINT users_email_check CHECK (email <> '');
COMMENT ON CONSTRAINT users_email_check ON app.users IS 'Reject empty emails';
