-- z3rno Postgres backend — one-time bootstrap for restricted-privilege environments.
--
-- z3rno auto-provisions its required extensions on every startup
-- (CREATE EXTENSION IF NOT EXISTS), which works out of the box on most
-- self-hosted Postgres. Many *managed* Postgres providers (RDS, Cloud SQL,
-- etc.) restrict CREATE EXTENSION to a privileged role — if z3rno's startup
-- log names a missing extension and points you here, ask whoever holds
-- that privileged role on your instance to run this script once. z3rno's
-- own runtime role does not need to be, and should not be, the role that
-- runs this.
--
-- Safe to re-run: every statement is idempotent.

CREATE EXTENSION IF NOT EXISTS vector;
CREATE EXTENSION IF NOT EXISTS age;

-- Makes Postgres preload AGE for every session on this database, so an
-- ordinary (non-superuser) app role can call cypher() without itself
-- running `LOAD 'age'` — that command requires superuser, so without this
-- z3rno's runtime role, running as anything less than superuser (as it
-- should), would fail to use the graph backend at all. `ALTER DATABASE`
-- takes a literal name, not an expression — this DO block reads
-- current_database() dynamically so the rest of this script doesn't need
-- editing to name the database explicitly.
DO $$
BEGIN
    EXECUTE format(
        'ALTER DATABASE %I SET session_preload_libraries = ''age''',
        current_database()
    );
END
$$;

-- z3rno's per-tenant-AGE-graph design (see engine/src/backend/postgres/
-- graph.rs's module doc) means the app's *runtime* role creates a new
-- graph (ag_catalog.create_graph()) the first time it ever sees a given
-- tenant — an ongoing operational need, not a one-time migration step.
-- Replace <app_role> below with your actual runtime role name.
--
-- GRANT USAGE ON SCHEMA ag_catalog TO <app_role>;
-- GRANT SELECT ON ALL TABLES IN SCHEMA ag_catalog TO <app_role>;
-- GRANT EXECUTE ON ALL FUNCTIONS IN SCHEMA ag_catalog TO <app_role>;
-- GRANT CREATE ON DATABASE current_database() TO <app_role>;
