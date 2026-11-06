use assert_cmd::Command;

#[test]
fn help_explains_auth_search_download_and_refresh_without_authenticating() {
    let output = Command::new(assert_cmd::cargo::cargo_bin!("gphotos"))
        .arg("--help")
        .output()
        .unwrap();
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    for phrase in [
        "auth chrome",
        "--person",
        "download PHOTO_ID",
        "GPHOTOS_SESSION",
        "inspect PHOTO_ID",
        "auth export",
        "session stays valid",
        "help search",
        "--all",
        "--location",
        "--from",
    ] {
        assert!(text.contains(phrase), "missing {phrase}");
    }
    for command in [
        vec!["help", "search"],
        vec!["auth", "chrome", "--help"],
        vec!["download", "--help"],
        vec!["inspect", "--help"],
        vec!["people", "--help"],
        vec!["auth", "export", "--help"],
        vec!["auth", "import", "--help"],
    ] {
        Command::new(assert_cmd::cargo::cargo_bin!("gphotos"))
            .args(command)
            .assert()
            .success();
    }
}
#[test]
fn invalid_inputs_fail_before_accessing_keychain() {
    for args in [
        vec!["search"],
        vec!["search", "beach", "--limit", "0"],
        vec!["download", "../../secret"],
        vec!["inspect", "../../secret"],
        vec!["auth", "export"],
    ] {
        Command::new(assert_cmd::cargo::cargo_bin!("gphotos"))
            .args(args)
            .assert()
            .failure();
    }
}

#[test]
fn detailed_help_documents_filters_pagination_metadata_and_session_refresh() {
    for (args, phrases) in [
        (
            vec!["help", "search"],
            vec![
                "AND",
                "--page-token",
                "--all",
                "YYYY-MM-DD",
                "--location",
                "--tag",
            ],
        ),
        (
            vec!["help", "inspect"],
            vec![
                "filename",
                "caption",
                "lens",
                "latitude/longitude",
                "timezone",
            ],
        ),
        (
            vec!["help", "auth"],
            vec![
                "No single guaranteed expiry",
                "refresh token",
                "auth cookies",
                "auth export",
            ],
        ),
    ] {
        let out = Command::new(assert_cmd::cargo::cargo_bin!("gphotos"))
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success());
        let text = String::from_utf8(out.stdout).unwrap();
        for phrase in phrases {
            assert!(text.contains(phrase), "help missing {phrase}");
        }
    }
}
