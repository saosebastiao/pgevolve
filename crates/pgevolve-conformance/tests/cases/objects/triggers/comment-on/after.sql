-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.notifications (
  id      bigint NOT NULL,
  message text   NOT NULL,
  CONSTRAINT notifications_pkey PRIMARY KEY (id)
);
CREATE FUNCTION app.send_notification() RETURNS trigger
    LANGUAGE plpgsql
AS $$
BEGIN
  RETURN NEW;
END
$$;
CREATE TRIGGER trg_notifications_send
  AFTER INSERT ON app.notifications
  FOR EACH ROW
  EXECUTE FUNCTION app.send_notification();
COMMENT ON TRIGGER trg_notifications_send ON app.notifications IS 'fires after each row insert to dispatch a notification';
