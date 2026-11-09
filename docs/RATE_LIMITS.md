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
- If a request fails with code 8, wait before retrying. For HTTP 429, follow
  the `Retry-After` delay if the CLI reports one. If you already have a page
  token, rerun with the same query, filters, account, and sort, plus
  `--page-token TOKEN`. A failure before the first page yields no token.

## CLI limits

| Setting | Value |
| --- | --- |
| Spacing between web RPCs | At least 350 ms within one CLI process |
| Connection timeout | 20 seconds |
| HTTP request timeout | 120 seconds |
| Search time budget | 120 seconds by default, adjustable with `--max-seconds` |
| Normal search | Up to 50 pages, 20 results by default, and 100 candidates when local filtering is needed |
| Search with `--all` | Up to 1,000 pages and 10,000 results by default, subject to the time budget |

The search time budget is checked between requests and an in-flight request can
run past it. When the CLI stops at a search budget, it reports an incomplete
result and a continuation token if more results may be available. These budgets
are local limits. Google can limit requests independently.

Google publishes separate [Picker and Library API quotas](https://developers.google.com/photos/overview/api-limits-quotas)
for OAuth projects. Those quotas apply to the public developer APIs. This CLI
uses the browser's search endpoint, whose quota is unknown.
