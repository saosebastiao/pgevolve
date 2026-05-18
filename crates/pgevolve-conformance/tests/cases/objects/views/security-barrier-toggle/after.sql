-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.accounts (
  id bigint NOT NULL,
  owner text,
  CONSTRAINT accounts_pkey PRIMARY KEY (id)
);
-- Toggle security_barrier to true
CREATE VIEW app.secure_view WITH (security_barrier = true) AS
  SELECT id, owner FROM app.accounts;
