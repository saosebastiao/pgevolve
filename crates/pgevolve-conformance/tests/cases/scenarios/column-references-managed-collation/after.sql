-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE COLLATION app.ci (provider = libc, locale = 'C');
CREATE TABLE app.users (
  id bigint NOT NULL,
  email text COLLATE app.ci NOT NULL,
  CONSTRAINT users_pkey PRIMARY KEY (id)
);
