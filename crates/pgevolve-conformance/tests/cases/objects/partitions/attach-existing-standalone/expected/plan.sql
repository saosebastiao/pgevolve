-- @pgevolve plan id=07215e840a0b7e9c version=0.4.1 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=attach_partition destructive=false targets=app.readings_2024
ALTER TABLE app.readings ATTACH PARTITION app.readings_2024 FOR VALUES FROM ('2024-01-01') TO ('2025-01-01');
COMMIT;

