CREATE VIEW app.active_users AS SELECT id, name FROM app.users WHERE active = true;
