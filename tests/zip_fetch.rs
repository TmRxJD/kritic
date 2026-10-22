//! A fixed real deflated ZIP tests manual entry copying independently of ZipWriter APIs.
use base64::{Engine, engine::general_purpose::STANDARD};
use kritic::cli::fetch::extract_zip;
use std::path::{Path, PathBuf};

const ARCHIVE: &str = concat!(
    "UEsDBBQAAAAIAA+dUV1ikbLBIAAAAB4AAAAWAAAAcmVsZWFzZS9iaW4va3JpdGljLmJpbitKTcxRSElNy0ksSU1RSCxKzsgsS1VIqixJLWb4zwUA",
    "UEsDBBQAAAAIAA+dUV0p3EL5DgAAAAwAAAALAAAAaWdub3JlZC50eHTLyy9RKE7NSU0uSU0BAFBLAQIUABQAAAAIAA+dUV1ikbLBIAAAAB4AAAAW",
    "AAAAAAAAAAAAAACAAQAAAAByZWxlYXNlL2Jpbi9rcml0aWMuYmluUEsBAhQAFAAAAAgAD51RXSncQvkOAAAADAAAAAsAAAAAAAAAAAAAAIABVAAAAGln",
    "bm9yZWQudHh0UEsFBgAAAAACAAIAfQAAAIsAAAAAAA=="
);

fn archive(directory: &Path) -> PathBuf {
    let path = directory.join("fixture.zip");
    std::fs::write(&path, STANDARD.decode(ARCHIVE).unwrap()).unwrap();
    path
}

#[test]
fn extracts_selected_deflated_entry_bytes_without_copying_unselected_entries() {
    let directory = tempfile::tempdir().unwrap();
    let zip = archive(directory.path());
    let output = directory.path().join("extracted");
    let written = extract_zip(&zip, &output, |name| {
        (name == "release/bin/kritic.bin").then(|| PathBuf::from("bin/selected.bin"))
    })
    .unwrap();
    assert_eq!(written, [output.join("bin/selected.bin")]);
    assert_eq!(
        std::fs::read(&written[0]).unwrap(),
        b"real deflated archive bytes\0\xff\n"
    );
    assert!(!output.join("ignored.txt").exists());
    assert!(!output.join("release").exists());
}

#[test]
fn rejects_a_selected_destination_that_escapes_the_output_directory() {
    let directory = tempfile::tempdir().unwrap();
    let zip = archive(directory.path());
    let output = directory.path().join("extracted");
    let error = extract_zip(&zip, &output, |name| {
        (name == "release/bin/kritic.bin").then(|| PathBuf::from("../escaped.bin"))
    })
    .unwrap_err();
    assert!(error.contains("refusing archive entry outside the target folder"));
    assert!(!directory.path().join("escaped.bin").exists());
    assert!(!output.exists());
}
