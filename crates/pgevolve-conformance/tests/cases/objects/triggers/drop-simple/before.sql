-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.accounts (
  id      bigint NOT NULL,
  balance numeric NOT NULL,
  CONSTRAINT accounts_pkey PRIMARY KEY (id)
);
CREATE FUNCTION app.validate_balance() RETURNS trigger
    LANGUAGE plpgsql
AS $$
BEGIN
  RETURN NEW;
END
$$;
CREATE TRIGGER trg_accounts_validate
  BEFORE UPDATE ON app.accounts
  FOR EACH ROW
  EXECUTE FUNCTION app.validate_balance();
