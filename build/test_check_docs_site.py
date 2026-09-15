#!/usr/bin/env python3
"""Tests for check_docs_site.py against small synthetic staged trees."""

from pathlib import Path

import pytest

from check_docs_site import check_site

ORIGIN = "https://example.com"


def write(path: Path, content: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(content, encoding="utf-8")


def build_base_tree(root: Path) -> Path:
    """A well-formed staged tree shaped like publish-docs.yml's output."""
    write(root / "CNAME", "example.com\n")

    write(
        root / "robots.txt",
        "User-agent: *\n"
        "Allow: /\n\n"
        f"Sitemap: {ORIGIN}/sitemap.xml\n"
        f"Sitemap: {ORIGIN}/docs/sitemap.xml\n",
    )

    write(
        root / "sitemap.xml",
        '<?xml version="1.0" encoding="UTF-8"?>\n'
        '<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">\n'
        f"  <url><loc>{ORIGIN}/</loc></url>\n"
        f"  <url><loc>{ORIGIN}/other/</loc></url>\n"
        "</urlset>\n",
    )
    write(
        root / "index.html",
        f'<html><head><link rel="canonical" href="{ORIGIN}/"></head><body></body></html>',
    )
    write(root / "other" / "index.html", "<html><body>other</body></html>")

    write(
        root / "docs" / "sitemap.xml",
        '<?xml version="1.0" encoding="UTF-8"?>\n'
        '<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">\n'
        f"  <url><loc>{ORIGIN}/docs/</loc></url>\n"
        f"  <url><loc>{ORIGIN}/docs/page/</loc></url>\n"
        "</urlset>\n",
    )
    write(root / "docs" / "feed_rss_created.xml", "<rss></rss>")
    write(
        root / "docs" / "index.html",
        f'<html><head><link rel="canonical" href="{ORIGIN}/docs/">'
        '<link rel="alternate" type="application/rss+xml" title="RSS feed" href="feed_rss_created.xml">'
        "</head><body></body></html>",
    )
    write(
        root / "docs" / "page" / "index.html",
        f'<html><head><link rel="canonical" href="{ORIGIN}/docs/page/">'
        '<link rel="alternate" type="application/rss+xml" title="RSS feed" href="../feed_rss_created.xml">'
        "</head><body></body></html>",
    )
    write(
        root / "docs" / "404.html",
        "<html><head>"
        '<link rel="alternate" type="application/rss+xml" title="RSS feed" href="/docs/feed_rss_created.xml">'
        "</head><body>not found</body></html>",
    )

    write(
        root / "llms.txt",
        "# Example\n\n"
        "> Summary.\n\n"
        "## Start here\n\n"
        f"- [Docs]({ORIGIN}/docs/): the documentation.\n"
        f"- [A page]({ORIGIN}/docs/page/): one page.\n"
        "- [Source](https://github.com/example/example): off-site, not checked.\n",
    )

    return root


def test_well_formed_tree_passes(tmp_path):
    root = build_base_tree(tmp_path)
    assert check_site(root) == []


def test_sitemap_loc_target_missing_fails(tmp_path):
    root = build_base_tree(tmp_path)
    write(
        root / "docs" / "sitemap.xml",
        '<?xml version="1.0" encoding="UTF-8"?>\n'
        '<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">\n'
        f"  <url><loc>{ORIGIN}/docs/</loc></url>\n"
        f"  <url><loc>{ORIGIN}/docs/missing/</loc></url>\n"
        "</urlset>\n",
    )
    failures = check_site(root)
    assert any(f"{ORIGIN}/docs/missing/" in f for f in failures)


def test_empty_sitemap_fails(tmp_path):
    root = build_base_tree(tmp_path)
    write(
        root / "docs" / "sitemap.xml",
        '<?xml version="1.0" encoding="UTF-8"?>\n'
        '<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">\n'
        "</urlset>\n",
    )
    failures = check_site(root)
    assert any(
        str(root / "docs" / "sitemap.xml") in f and "no <loc>" in f for f in failures
    )


def test_missing_canonical_tag_fails(tmp_path):
    root = build_base_tree(tmp_path)
    write(
        root / "docs" / "page" / "index.html",
        '<html><head>'
        '<link rel="alternate" type="application/rss+xml" title="RSS feed" href="../feed_rss_created.xml">'
        "</head><body></body></html>",
    )
    failures = check_site(root)
    assert any(
        str(root / "docs" / "page" / "index.html") in f and "canonical" in f
        for f in failures
    )


def test_404_html_exempt_from_missing_canonical(tmp_path):
    root = build_base_tree(tmp_path)
    # docs/404.html in the base tree already has no canonical tag; confirm it
    # is exempted rather than flagged.
    failures = check_site(root)
    assert not any(str(root / "docs" / "404.html") in f for f in failures)


def test_canonical_pointing_elsewhere_fails(tmp_path):
    root = build_base_tree(tmp_path)
    write(
        root / "docs" / "page" / "index.html",
        f'<html><head><link rel="canonical" href="{ORIGIN}/docs/other-page/">'
        '<link rel="alternate" type="application/rss+xml" title="RSS feed" href="../feed_rss_created.xml">'
        "</head><body></body></html>",
    )
    failures = check_site(root)
    assert any("canonical" in f and "page" in f for f in failures)


def test_duplicate_loc_across_sitemaps_fails(tmp_path):
    root = build_base_tree(tmp_path)
    write(
        root / "sitemap.xml",
        '<?xml version="1.0" encoding="UTF-8"?>\n'
        '<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">\n'
        f"  <url><loc>{ORIGIN}/</loc></url>\n"
        f"  <url><loc>{ORIGIN}/other/</loc></url>\n"
        f"  <url><loc>{ORIGIN}/docs/</loc></url>\n"
        "</urlset>\n",
    )
    failures = check_site(root)
    assert any(
        f"{ORIGIN}/docs/" in f and str(root / "sitemap.xml") in f and str(root / "docs" / "sitemap.xml") in f
        for f in failures
    )


def test_sitemap_missing_from_robots_fails(tmp_path):
    root = build_base_tree(tmp_path)
    write(
        root / "robots.txt",
        "User-agent: *\nAllow: /\n\n" f"Sitemap: {ORIGIN}/sitemap.xml\n",
    )
    failures = check_site(root)
    assert any("robots.txt" in f and "docs" in f for f in failures)


def test_relative_feed_link_broken_fails(tmp_path):
    root = build_base_tree(tmp_path)
    write(
        root / "docs" / "page" / "index.html",
        f'<html><head><link rel="canonical" href="{ORIGIN}/docs/page/">'
        '<link rel="alternate" type="application/rss+xml" title="RSS feed" href="../nonexistent_feed.xml">'
        "</head><body></body></html>",
    )
    failures = check_site(root)
    assert any("nonexistent_feed.xml" in f for f in failures)


def test_root_absolute_feed_link_resolves_and_breaks_correctly(tmp_path):
    root = build_base_tree(tmp_path)
    # The well-formed base tree's docs/404.html already carries a correct
    # root-absolute feed href -- test_well_formed_tree_passes pins that. Here,
    # break it and confirm the same root-relative resolution rule catches it.
    write(
        root / "docs" / "404.html",
        "<html><head>"
        '<link rel="alternate" type="application/rss+xml" title="RSS feed" href="/docs/nonexistent_feed.xml">'
        "</head><body>not found</body></html>",
    )
    failures = check_site(root)
    assert any("/docs/nonexistent_feed.xml" in f for f in failures)


if __name__ == "__main__":
    raise SystemExit(pytest.main([__file__, "-v"]))


def test_missing_llms_txt_fails(tmp_path):
    root = build_base_tree(tmp_path)
    (root / "llms.txt").unlink()
    failures = check_site(root)
    assert any("llms.txt does not exist" in f for f in failures)


def test_llms_txt_link_to_missing_page_fails(tmp_path):
    root = build_base_tree(tmp_path)
    write(
        root / "llms.txt",
        "# Example\n\n"
        "> Summary.\n\n"
        f"- [Moved]({ORIGIN}/docs/moved/): this page is not in the tree.\n",
    )
    failures = check_site(root)
    assert any(f"{ORIGIN}/docs/moved/" in f for f in failures)


def test_llms_txt_offsite_links_are_not_checked(tmp_path):
    root = build_base_tree(tmp_path)
    write(
        root / "llms.txt",
        "# Example\n\n"
        "> Summary.\n\n"
        "- [Elsewhere](https://github.com/example/nope): off-site.\n"
        "- [Also elsewhere](https://crates.io/crates/nope): off-site.\n",
    )
    assert check_site(root) == []
