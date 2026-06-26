use std::fs;
use std::path::PathBuf;

fn project_file(relative_path: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(relative_path);
    fs::read_to_string(&path).unwrap_or_else(|err| {
        panic!(
            "failed to read {}: {err}",
            path.strip_prefix(env!("CARGO_MANIFEST_DIR"))
                .unwrap_or(&path)
                .display()
        )
    })
}

#[test]
fn desktop_entry_accepts_parquet_files() {
    let desktop = project_file("packaging/dev.parquetta.Parquetta.desktop");

    assert!(desktop.contains("Exec=parquetta %F"));
    assert!(desktop.contains("MimeType=application/vnd.apache.parquet;application/x-parquet;"));
}

#[test]
fn shared_mime_database_declares_parquet_glob() {
    let mime = project_file("packaging/dev.parquetta.Parquetta.mime.xml");

    assert!(mime.contains(r#"<mime-type type="application/vnd.apache.parquet">"#));
    assert!(mime.contains(r#"<glob pattern="*.parquet"/>"#));
    assert!(mime.contains(r#"<alias type="application/x-parquet"/>"#));
}

#[test]
fn appstream_metadata_advertises_parquet_media_types() {
    let metainfo = project_file("packaging/dev.parquetta.Parquetta.metainfo.xml");

    assert!(metainfo
        .contains("<launchable type=\"desktop-id\">dev.parquetta.Parquetta.desktop</launchable>"));
    assert!(metainfo.contains("<mediatype>application/vnd.apache.parquet</mediatype>"));
    assert!(metainfo.contains("<mediatype>application/x-parquet</mediatype>"));
}

#[test]
fn deb_and_rpm_packages_install_mime_definition() {
    let cargo_toml = project_file("Cargo.toml");

    assert!(cargo_toml.contains(
        r#"["packaging/dev.parquetta.Parquetta.mime.xml", "usr/share/mime/packages/dev.parquetta.Parquetta.xml", "644"]"#
    ));
    assert!(cargo_toml.contains(
        r#"{ source = "packaging/dev.parquetta.Parquetta.mime.xml", dest = "/usr/share/mime/packages/dev.parquetta.Parquetta.xml", mode = "644" }"#
    ));
}
