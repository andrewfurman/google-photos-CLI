# Read-only protocol mappings

Google Photos uses undocumented `batchexecute` RPCs. These mappings are observed
behavior, not a Google-supported contract. No captured account data belongs here.
The baseline framing and metadata positions are credited in the third-party notices.

| Operation | RPC | Payload / returned fields |
| --- | --- | --- |
| Keyword search | `EzkLib` | Query in payload `[7][0]`; result mode `[6]=2` selects the most-relevant set. The legacy three-field query form is not used. |
| Named face groups | `yQelMe` | `[null,null,null,[[1,null,1]]]`; section type 1 under response `[1]` contains face groups. |
| People-only search | `EzkLib` | Selected face-group ID in filter `[7][1][0][0][0][0][1]`; mode 1 matches the People & Pets navigation's recent results. |
| Keyword plus person | `EzkLib` | Both the query and face-group ID are present in the same filter; keyword mode 2 selects relevance. |
| Extended photo info | `fDcn4b` | `[photo_id,1,null,null,1]`; the photo record is response `[0]`. |
| Photo / download info | `VrseUb` | `[photo_id,null,null,null,null]`; direct full-size/original URLs are response `[1]` and `[7]` when present. |
| Batched filenames | `EWgK9e` | Options require their own nested array; response `[0][1]` contains filename records. |

Extended-photo record positions used by `inspect`: filename 2, capture timestamp 3,
timezone offset 4, bytes 5, dimensions 6–7, coordinates 9/13, people 18, albums 19,
camera 23, upload source 27, and storage attributes 30. Camera positions 0/1 are
manufacturer/model; 3–6 are focal length, f-number, ISO, and exposure seconds.
Coordinate integers are scaled by 1e7.

A per-photo face-group entry holds the group ID at `[0][0]` and the assigned name
at `[1][5]`. Unnamed groups are retained. Owner/contact records elsewhere in the
response are **not face tags**. Missing face-group metadata is reported as
`people_status=not_returned`, rather than inferring the photo contains no people.

Parsers support sparse trailing field objects. Tests must use synthetic values
and cover missing/unknown fields, malformed data, and identity boundaries. For
live verification, compare requested group IDs against each inspected result's
tags and compare a keyword result with the current web UI. Never publish live
RPC captures or signed media URLs.
