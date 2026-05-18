CREATE DOMAIN app.positive_int AS integer NOT NULL DEFAULT 1 CONSTRAINT positive_int_check CHECK (VALUE > 0);
