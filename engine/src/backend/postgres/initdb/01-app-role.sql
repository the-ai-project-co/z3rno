-- Dev-only: a non-superuser role for tests to connect as. Runs once, at
-- container first-init (docker-entrypoint-initdb.d), so it's never subject
-- to the CREATE ROLE race a per-test "create if missing" DO block would
-- hit under parallel test execution.
--
-- This matters beyond convenience: the default `postgres` role in this dev
-- image is a superuser, and Postgres superusers unconditionally bypass RLS
-- regardless of FORCE ROW LEVEL SECURITY — that's Postgres's own semantics,
-- not something policy SQL can override. Tests that connect as `postgres`
-- can't actually prove tenant isolation holds; they just don't hit it.
--
-- Creates the extensions itself, before granting on `ag_catalog` — that
-- schema doesn't exist until `CREATE EXTENSION age` creates it, so grants
-- on it can't come first. `provision::ensure_extensions` is idempotent and
-- just no-ops on these when the engine starts against this instance later.
CREATE EXTENSION IF NOT EXISTS vector;
CREATE EXTENSION IF NOT EXISTS age;
-- Also set here (not just left to `provision::ensure_extensions`, which
-- some test files never call because they don't touch AGE) so it's active
-- for every connection to this database from the start, regardless of
-- which test file happens to run first.
ALTER DATABASE z3rno_dev SET session_preload_libraries = 'age';

CREATE ROLE z3rno_app LOGIN PASSWORD 'z3rno_app' NOSUPERUSER NOBYPASSRLS;
GRANT ALL ON SCHEMA public TO z3rno_app;
ALTER DEFAULT PRIVILEGES IN SCHEMA public GRANT ALL ON TABLES TO z3rno_app;

-- The per-tenant-AGE-graph design (see graph.rs's module doc) means the
-- *runtime* app role creates a new graph — `ag_catalog.create_graph()` —
-- the first time it ever sees a given tenant, not just once at startup
-- provisioning. So this role needs ongoing CREATE on the database, not a
-- one-time grant a migration could satisfy and revoke — a real operational
-- requirement of this design, not dev-fixture-only noise. A production DBA
-- running `bootstrap.sql` for a restricted app role needs the equivalent
-- of these four grants too.
GRANT USAGE ON SCHEMA ag_catalog TO z3rno_app;
GRANT SELECT ON ALL TABLES IN SCHEMA ag_catalog TO z3rno_app;
GRANT EXECUTE ON ALL FUNCTIONS IN SCHEMA ag_catalog TO z3rno_app;
GRANT CREATE ON DATABASE z3rno_dev TO z3rno_app;
