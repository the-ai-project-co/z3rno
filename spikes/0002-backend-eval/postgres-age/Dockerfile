# apache/age ships Postgres 18 + AGE preinstalled, but no pgvector.
# pgvector IS packaged for pg18 in the pgdg apt repo that's already configured
# in this base image, so this is a one-line `apt-get install` — no source build needed.
FROM apache/age:latest

RUN apt-get update \
    && apt-get install -y --no-install-recommends postgresql-18-pgvector \
    && rm -rf /var/lib/apt/lists/*
