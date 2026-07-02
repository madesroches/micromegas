use micromegas_object_cache::validation::validate_key;

fn prefixes(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

#[test]
fn rejects_structurally_invalid_keys() {
    let allowed = prefixes(&["blobs"]);
    assert!(validate_key("", &allowed).is_err());
    assert!(validate_key("/blobs/x", &allowed).is_err());
    assert!(validate_key("blobs/../secret", &allowed).is_err());
    // Traversal is rejected even in allow-all mode.
    assert!(validate_key("a/../b", &[]).is_err());
}

#[test]
fn admits_key_under_any_configured_prefix() {
    let allowed = prefixes(&["blobs", "views"]);
    assert!(validate_key("blobs/p/s/b", &allowed).is_ok());
    assert!(validate_key("views/v/x.parquet", &allowed).is_ok());
    // A key equal to a prefix is admitted.
    assert!(validate_key("blobs", &allowed).is_ok());
    assert!(validate_key("views", &allowed).is_ok());
}

#[test]
fn rejects_key_outside_all_prefixes() {
    let allowed = prefixes(&["blobs", "views"]);
    assert!(validate_key("secret/x", &allowed).is_err());
    assert!(validate_key("blob/x", &allowed).is_err());
}

#[test]
fn enforces_segment_boundary_per_prefix() {
    let allowed = prefixes(&["blobs"]);
    // `blobs-secret` shares a textual prefix but not a path boundary.
    assert!(validate_key("blobs-secret/y", &allowed).is_err());
    assert!(validate_key("blobsx", &allowed).is_err());
}

#[test]
fn empty_list_allows_all_valid_keys() {
    // Reachable only via --allow-all-prefixes; still enforces structure.
    assert!(validate_key("anything/goes/here", &[]).is_ok());
    assert!(validate_key("secret/x", &[]).is_ok());
}
