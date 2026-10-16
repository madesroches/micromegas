use micromegas_object_cache::metric_tags::{PREFIX_OTHER, longest_prefix_match};

#[test]
fn empty_labels_never_match() {
    assert_eq!(longest_prefix_match(&[], "blobs/x"), None);
}

#[test]
fn exact_and_slash_boundary_match() {
    let labels = ["blobs", "views"];
    assert_eq!(longest_prefix_match(&labels, "blobs"), Some(0));
    assert_eq!(longest_prefix_match(&labels, "blobs/x"), Some(0));
    assert_eq!(longest_prefix_match(&labels, "views/y/z"), Some(1));
}

#[test]
fn non_boundary_prefix_does_not_match() {
    // "blobs-secret" must not match label "blobs": it isn't the label
    // itself, nor is it "blobs/" + something.
    let labels = ["blobs"];
    assert_eq!(longest_prefix_match(&labels, "blobs-secret"), None);
    assert_eq!(longest_prefix_match(&labels, "unrelated"), None);
}

#[test]
fn longest_match_wins_among_nested_labels() {
    let labels = ["blobs", "blobs/nested"];
    assert_eq!(longest_prefix_match(&labels, "blobs/nested/x"), Some(1));
    assert_eq!(longest_prefix_match(&labels, "blobs/other"), Some(0));
}

#[test]
fn allow_all_style_empty_list_falls_back_to_other() {
    // Mirrors `RangeCache::new`'s default (no `with_prefix_labels` call): an
    // empty label list means every key classifies as `PREFIX_OTHER`.
    let labels: [&str; 0] = [];
    assert_eq!(longest_prefix_match(&labels, "anything"), None);
    assert_eq!(PREFIX_OTHER, "other");
}
