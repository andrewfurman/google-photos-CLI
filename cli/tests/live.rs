//! Opt-in only: uses your own saved Google Photos session. CI never enables this.
use anyhow::{ensure, Context, Result};
use google_photos_cli::{
    auth,
    client::{PhotosClient, SearchOptions},
    search::{compose_query, DateRange},
};
use serde_json::json;
use std::collections::HashSet;

#[test]
#[ignore = "Requires your explicit live session and GPHOTOS_LIVE_* test inputs; see docs/TESTING.md"]
fn search_tactics_against_your_own_library() -> Result<()> {
    let setting = |key: &str| {
        std::env::var(key)
            .with_context(|| format!("Set {key} to a known value in your own library"))
    };
    let names = vec![
        setting("GPHOTOS_LIVE_PERSON_A")?,
        setting("GPHOTOS_LIVE_PERSON_B")?,
    ];
    let keyword = setting("GPHOTOS_LIVE_KEYWORD")?;
    let location = setting("GPHOTOS_LIVE_LOCATION")?;
    let from = setting("GPHOTOS_LIVE_FROM")?;
    let to = setting("GPHOTOS_LIVE_TO")?;
    let minimum = std::env::var("GPHOTOS_LIVE_MIN_RESULTS")
        .unwrap_or_else(|_| "1".into())
        .parse::<usize>()?;
    let mut client = PhotosClient::connect(auth::load()?)?;
    let result = (|| -> Result<()> {
        let options = SearchOptions {
            limit: 100,
            all: true,
            max_seconds: 120,
            ..SearchOptions::default()
        };
        let first = client.search_people("", &names, &[], options.clone(), None)?;
        ensure!(
            first.items.len() >= minimum,
            "The selected people did not return the expected minimum matches"
        );
        ensure!(
            first.people.len() == 2,
            "Test people must resolve to distinct face groups"
        );
        let required = first
            .people
            .iter()
            .map(|p| p.id.as_str())
            .collect::<Vec<_>>();
        for chunk in first.items.chunks(20) {
            let values = client.rpc_batch(
                "fDcn4b",
                &chunk
                    .iter()
                    .map(|p| json!([p.id, 1, null, null, 1]))
                    .collect::<Vec<_>>(),
            )?;
            for (photo, metadata) in chunk.iter().zip(values) {
                ensure!(
                    metadata[0][0].as_str() == Some(photo.id.as_str()),
                    "Metadata was assigned to the wrong result"
                );
                let tags = metadata[0][18]
                    .as_array()
                    .context("A returned match has no verifiable person tags")?;
                for id in &required {
                    ensure!(
                        tags.iter().any(|p| p[0][0].as_str() == Some(*id)),
                        "A result did not contain every requested face group"
                    );
                }
            }
        }
        if let Some(token) = &first.next_page_token {
            let next = client.search_people(
                "",
                &names,
                &[],
                SearchOptions {
                    limit: 5,
                    ..options.clone()
                },
                Some(token),
            )?;
            let seen = first
                .items
                .iter()
                .map(|p| p.id.as_str())
                .collect::<HashSet<_>>();
            ensure!(
                next.items.iter().all(|p| !seen.contains(p.id.as_str())),
                "Pagination repeated previously returned photos"
            );
        }
        let text = client.search(&keyword, 3, None)?;
        ensure!(!text.items.is_empty(), "Known keyword returned no results");
        let place_query = compose_query(None, Some(&location), &[]);
        let place = client.search(&place_query, 3, None)?;
        ensure!(
            place.query == location,
            "The place name was changed before sending it to Google"
        );
        ensure!(
            !place.items.is_empty(),
            "Known location returned no results"
        );
        let dates = DateRange::parse(Some(&from), Some(&to))?;
        let dated = client.search_people(
            "",
            &[],
            &[],
            SearchOptions {
                limit: 20,
                all: true,
                dates: dates.clone(),
                ..SearchOptions::default()
            },
            None,
        )?;
        ensure!(
            !dated.items.is_empty(),
            "Known date range returned no results"
        );
        ensure!(
            dated
                .items
                .iter()
                .all(|p| dates.matches(p.taken_at.as_deref()) == Some(true)),
            "An out-of-range photo escaped the date filter"
        );
        let inspected = client.inspect(&first.items[0].id)?;
        let value = serde_json::to_value(inspected)?;
        for key in [
            "filename",
            "caption",
            "description",
            "taken_at",
            "uploaded_at",
            "camera",
            "location",
            "people",
        ] {
            ensure!(value.get(key).is_some(), "Photo inspection omitted {key}");
        }
        println!("Verified {} all-person results, pagination, keyword/location/date searches, and metadata shape. No live account data printed.",first.items.len());
        Ok(())
    })();
    let saved = client.save();
    result?;
    saved
}
