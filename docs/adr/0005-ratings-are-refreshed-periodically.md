# Ratings are refreshed periodically, everything else once

Descriptive information is fetched from TMDB once and then only when an admin asks (spec #1), because
names and descriptions rarely change and manual fixes must stick. The Rating (TMDB's user score and
vote count) changes all the time, so it is the one exception: each Movie, Show and Episode records when
its Rating was last fetched, and a background sweep in the asset server refreshes every Rating older
than a maximum age (default 7 days), a few at a time. Episodes are refreshed in bulk through their
Season's details. A Rating is never a Locked field.

## Considered Options

- **TMDB's change feed** (`/movie/changes`, `/tv/changes`): new votes do not appear in it, so Ratings
  would never update.
- **Refreshing when an entry is read**: browsing would depend on TMDB's availability and latency, which
  the design already rules out for images.
- **The planned Scheduler service**: does not exist yet; the sweep can move there once it does.

## Consequences

About one request per Movie, per Show and per Season per maximum age; a library of 500 Movies and 50
Shows of 5 Seasons costs roughly 800 requests a week, far below TMDB's limits. Outages are retried
like any other lookup.
