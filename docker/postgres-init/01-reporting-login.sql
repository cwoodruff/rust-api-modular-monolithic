-- Runs once, when the compose volume is first created.
-- reporting_reader is the read-only group role the Reporting migration grants to (ADR-0014); the
-- migration creates it too if it is missing. This adds the login the API uses for
-- ConnectionStrings:Reporting. The password is for this local container only.
DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'reporting_reader') THEN
        CREATE ROLE reporting_reader NOLOGIN;
    END IF;
    IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'reporting') THEN
        CREATE ROLE reporting LOGIN PASSWORD 'reporting' IN ROLE reporting_reader;
    END IF;
END
$$;
