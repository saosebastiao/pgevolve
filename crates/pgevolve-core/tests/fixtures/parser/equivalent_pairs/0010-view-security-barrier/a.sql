CREATE VIEW app.secure_v WITH (security_barrier = true) AS SELECT id FROM app.users;
