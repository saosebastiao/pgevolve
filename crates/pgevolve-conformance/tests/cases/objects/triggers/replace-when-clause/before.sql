-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.transactions (
  id     bigint NOT NULL,
  amount numeric NOT NULL,
  CONSTRAINT transactions_pkey PRIMARY KEY (id)
);
CREATE FUNCTION app.record_transaction() RETURNS trigger
    LANGUAGE plpgsql
AS $$
BEGIN
  RETURN NEW;
END
$$;
CREATE TRIGGER trg_transactions_record
  AFTER INSERT ON app.transactions
  FOR EACH ROW
  EXECUTE FUNCTION app.record_transaction();
