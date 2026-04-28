use crate::host_api::HostApi;
use std::fs;
use tempfile::tempdir;

#[test]
fn load_template_uses_configured_templates_dir() {
    let dir = tempdir().expect("tempdir");
    let templates_dir = dir.path().join("templates");
    fs::create_dir_all(templates_dir.join("moderation")).expect("create templates dir");
    fs::write(
        templates_dir.join("moderation").join("warn.txt"),
        "custom warn template",
    )
    .expect("write custom template");

    let api = HostApi::new(false).with_templates_dir(templates_dir);

    assert_eq!(api.load_template("moderation/warn"), "custom warn template");
}

#[test]
fn load_template_falls_back_to_bundled_templates() {
    let dir = tempdir().expect("tempdir");
    let api = HostApi::new(false).with_templates_dir(dir.path().join("missing"));
    let bundled = fs::read_to_string("bundled_templates/moderation/warn.txt")
        .expect("bundled template should exist");

    assert_eq!(api.load_template("moderation/warn"), bundled);
}
