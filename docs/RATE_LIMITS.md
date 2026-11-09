# Google Photos rate limits and timeouts

The CLI uses Google Photos' web endpoints. The figures below describe the CLI
and observed responses from those endpoints. Google did not disclose a numeric
search quota, how it is scoped, or when it resets.

## Observed on November 9, 2026

- During live testing, keyword searches in both sort modes began returning HTTP
  200 with a framed RPC error, code 8 (`RESOURCE_EXHAUSTED`). Multiple keywords
  failed while the signed-in session and people lookup still worked.
- Importing the Chrome session again did not restore keyword search. Both modes
  were still limited after a 45-second cooldown. The cooldown was a test
  interval. Google supplied no reset time.
## How searches retry

Searches retry RPC code 8 and HTTP 429. They honor `Retry-After` in seconds or as
an HTTP date. Without a usable header, waits start at 1 second, double after each
rejection, and cap at 30 seconds. If a wait would consume the remaining search
budget, the CLI stops and returns collected matches and a page token, even when
the first page was limited. If resolving a person's name takes the full budget,
retry with the same filters and reuse any token supplied to that command.

Resume with the same query, filters, account, and sort, plus `--page-token TOKEN`.
Searches may omit filenames when their time budget expires. Authentication setup
and media downloads have separate timeouts.

## CLI limits

| Setting | Value |
| --- | --- |
| Spacing between web RPCs | At least 350 ms within one CLI process |
| Connection timeout | 20 seconds |
| HTTP request timeout | 120 seconds |
| Search time budget | 120 seconds by default, adjustable with `--max-seconds` |
| Normal search | Up to 50 pages, 20 results by default, and 100 candidates when local filtering is needed |
| Search with `--all` | Up to 1,000 pages and 10,000 results by default, subject to the time budget |

The search budget covers search requests and retry delays. The CLI can stop early
when Google asks it to wait longer than the remaining budget. Google applies its
own limits independently.

Google publishes separate [Picker and Library API quotas](https://developers.google.com/photos/overview/api-limits-quotas)
for OAuth projects. Those quotas apply to the public developer APIs. This CLI
uses the browser's search endpoint, whose quota is unknown.
