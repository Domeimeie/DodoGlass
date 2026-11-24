# One prioritised queue for all TMDB requests

All background TMDB work of the asset server goes through one in-memory queue, worked off by one
worker behind one rate limiter (default at most 4 requests in flight and 20 per second, honouring
`Retry-After` on HTTP 429). Jobs have three priorities, first in, first out within each:

1. **New**: Movies, Shows, Seasons and Episodes that have no metadata yet.
2. **Retry**: lookups whose backoff has run out after TMDB was unreachable.
3. **Rating**: the periodic Rating refresh of ADR 0005.

So indexing new uploads is never delayed by a Rating sweep, and no part of the asset server can exceed
TMDB's limits on its own. A job that is queued again for the same entry moves up to the higher
priority instead of being queued twice. Admin actions (pick a Match, refresh) stay synchronous within
their HTTP request, but take the same limiter with top priority.

The database stays the source of truth: the queue is rebuilt from it at startup (what is due can be
read from the catalogue), so nothing is lost when the process stops. Episodes keep their own request
for their details and imdbID; only Ratings are read in bulk from Season details.

## Considered Options

- **Two independent loops** (matcher and Rating sweep, before this ADR): each paced only by TMDB's
  answers, with no shared limit and no priority, so a sweep could push new indexing into rate limiting.
- **A queue table in SQLite**: survives restarts, but duplicates state the catalogue already holds and
  can drift from it.
- **Newest first (LIFO) for new entries**: the first files of a large import would wait for the last.
- **Episodes from Season details only**: one request per Season instead of per Episode, but without the
  Episode imdbID; rejected to keep it immediate.
