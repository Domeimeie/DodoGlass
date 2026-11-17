# Diesel on SQLite first, Postgres later

The asset server uses Diesel with SQLite, and is meant to move to Postgres when the platform needs
more than one instance or shares a database with the user service and scheduler. Diesel does not make
that move free (migrations are raw SQL per backend; UUIDs, timestamps and `RETURNING` differ), so
migrations stay within SQL both backends accept. Until the move, only one asset server instance may
run.
