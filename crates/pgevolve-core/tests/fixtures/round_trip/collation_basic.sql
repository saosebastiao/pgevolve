-- Tier-3 round-trip fixture stub (created in Stage 3 of v0.3.8 collation plan).
--
-- NOT BLESSED yet: the parser lands in Stage 4 and the renderer lands in
-- Stage 6, so a round-trip would fail end-to-end before Stage 6 completes.
-- The Stage 6 implementer should wire this fixture into the round-trip
-- harness (none currently auto-loads .sql files from this directory) and
-- bless the expected output.
CREATE COLLATION app.case_insensitive (provider = icu, locale = 'und', deterministic = false);
