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
