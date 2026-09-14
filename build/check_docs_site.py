#!/usr/bin/env python3
"""Validate a staged micromegas.info site tree before it is published.

Checks the tree that ``publish-docs.yml`` assembles under ``public_docs/`` (or
any equivalent staging directory passed on the command line):

1. Every ``<loc>`` in every ``**/sitemap.xml`` under the root resolves to a
   file that exists.
2. No ``<loc>`` appears twice, within a sitemap or across sitemaps.
3. ``robots.txt`` exists at the root, every ``Sitemap:`` line resolves to an
   existing file, and the advertised set equals the set of ``sitemap.xml``
   files actually found.
4. Every ``<link rel="canonical">`` tag (scanned from ``<root>/docs/**/*.html``
   and ``<root>/index.html``) points at the file that emitted it.
5. Every feed autodiscovery link (``<link rel="alternate"
   type="application/rss+xml">``, from the same HTML files as check 4)
   resolves to a file that exists.

This validates the staged tree offline -- it needs no network access and runs
on every pull request that touches these paths, unlike curling the live site
after it has already shipped.
"""

import re
import sys
import xml.etree.ElementTree as ET
from pathlib import Path
from urllib.parse import unquote, urlsplit

SITEMAP_NS = "{http://www.sitemaps.org/schemas/sitemap/0.9}"

CANONICAL_RE = re.compile(
    r'<link\b[^>]*\brel=(["\'])canonical\1[^>]*>', re.IGNORECASE
)
FEED_RE = re.compile(
    r'<link\b[^>]*\brel=(["\'])alternate\1[^>]*\btype=(["\'])application/rss\+xml\2[^>]*>',
    re.IGNORECASE,
)
HREF_RE = re.compile(r'\bhref=(["\'])(.*?)\1', re.IGNORECASE)
SITEMAP_LINE_RE = re.compile(r"^Sitemap:\s*(\S+)\s*$", re.IGNORECASE | re.MULTILINE)


class SiteCheckError(Exception):
    """A malformed staged tree that makes the check itself impossible to run."""


def read_origin(root: Path) -> str:
    cname_path = root / "CNAME"
    if not cname_path.is_file():
        raise SiteCheckError(f"{cname_path} does not exist")
    host = cname_path.read_text(encoding="utf-8").strip()
    if not host:
        raise SiteCheckError(f"{cname_path} is empty")
    return f"https://{host}"


def _resolve_url_path(url_path: str) -> str:
    """Apply the sitemap/canonical convention: a directory URL serves index.html."""
    if url_path == "" or url_path.endswith("/"):
        return url_path + "index.html"
    return url_path


def url_to_path(root: Path, origin: str, url: str) -> Path:
    """Map an absolute URL under ``origin`` to the file it should resolve to."""
    if not url.startswith(origin + "/") and url != origin:
        raise ValueError(f"URL {url!r} is not under the configured origin {origin!r}")
    parsed = urlsplit(url)
    rel = _resolve_url_path(unquote(parsed.path))
    return root / rel.lstrip("/")


def href_to_path(root: Path, html_file: Path, href: str) -> Path:
    """Resolve a link href found in ``html_file`` to a file under ``root``.

    A root-absolute href (starting with ``/``) is resolved against the staged
    root, the same rule ``url_to_path`` applies to a root-relative path --
    ``build.py`` sets the 404 template's ``base_url`` to ``site_url``'s path,
    so its feed links are root-absolute rather than genuinely relative.
    """
    parsed = urlsplit(href)
    path = unquote(parsed.path)
    if href.startswith("/"):
        rel = _resolve_url_path(path)
        return root / rel.lstrip("/")
    return (html_file.parent / path).resolve()


def find_sitemaps(root: Path) -> list[Path]:
    return sorted(root.glob("**/sitemap.xml"))


def find_checked_html_files(root: Path) -> list[Path]:
    files = []
    index_html = root / "index.html"
    if index_html.is_file():
        files.append(index_html)
    docs_dir = root / "docs"
    if docs_dir.is_dir():
        files.extend(sorted(docs_dir.glob("**/*.html")))
    return files


def parse_sitemap_locs(sitemap_path: Path) -> list[str]:
    try:
        tree = ET.parse(sitemap_path)
    except ET.ParseError as e:
        raise SiteCheckError(f"{sitemap_path}: could not parse as XML: {e}")
    return [
        (loc_el.text or "").strip()
        for loc_el in tree.getroot().iter(f"{SITEMAP_NS}loc")
        if (loc_el.text or "").strip()
    ]


def extract_hrefs(html_text: str, tag_re: re.Pattern) -> list[str]:
    hrefs = []
    for tag_match in tag_re.finditer(html_text):
        href_match = HREF_RE.search(tag_match.group(0))
        if href_match:
            hrefs.append(href_match.group(2))
    return hrefs


def check_sitemap_locs_resolve(root: Path, origin: str, sitemaps: list[Path]) -> list[str]:
    failures = []
    for sitemap_path in sitemaps:
        for loc in parse_sitemap_locs(sitemap_path):
            try:
                target = url_to_path(root, origin, loc)
            except ValueError as e:
                failures.append(f"{sitemap_path}: {e}")
                continue
            if not target.is_file():
                failures.append(
                    f"{sitemap_path}: <loc>{loc}</loc> resolves to {target}, which does not exist"
                )
    return failures


def check_no_duplicate_locs(sitemaps: list[Path]) -> list[str]:
    failures = []
    seen: dict[str, Path] = {}
    for sitemap_path in sitemaps:
        for loc in parse_sitemap_locs(sitemap_path):
            if loc in seen:
                failures.append(
                    f"{loc!r} appears in both {seen[loc]} and {sitemap_path}"
                    if seen[loc] != sitemap_path
                    else f"{loc!r} appears twice in {sitemap_path}"
                )
            else:
                seen[loc] = sitemap_path
    return failures


def check_robots_txt(root: Path, origin: str, sitemaps: list[Path]) -> list[str]:
    failures = []
    robots_path = root / "robots.txt"
    if not robots_path.is_file():
        return [f"{robots_path} does not exist"]

    robots_text = robots_path.read_text(encoding="utf-8")
    advertised = SITEMAP_LINE_RE.findall(robots_text)
    if not advertised:
        failures.append(f"{robots_path} declares no 'Sitemap:' line")

    advertised_paths = set()
    for sitemap_url in advertised:
        try:
            target = url_to_path(root, origin, sitemap_url)
        except ValueError as e:
            failures.append(f"{robots_path}: {e}")
            continue
        if not target.is_file():
            failures.append(
                f"{robots_path}: Sitemap: {sitemap_url} resolves to {target}, which does not exist"
            )
        advertised_paths.add(target.resolve())

    found_paths = {p.resolve() for p in sitemaps}
    missing_from_robots = found_paths - advertised_paths
    for p in sorted(missing_from_robots):
        failures.append(f"{p} exists but is not advertised in {robots_path}")
    stale_in_robots = advertised_paths - found_paths
    for p in sorted(stale_in_robots):
        failures.append(f"{robots_path} advertises {p}, which is not a sitemap.xml file found under {root}")

    return failures


def check_canonical_tags(root: Path, origin: str, html_files: list[Path]) -> list[str]:
    failures = []
    for html_file in html_files:
        html_text = html_file.read_text(encoding="utf-8", errors="replace")
        hrefs = extract_hrefs(html_text, CANONICAL_RE)
        if not hrefs:
            continue  # e.g. 404.html, deliberately excluded
        for href in hrefs:
            try:
                target = url_to_path(root, origin, href)
            except ValueError as e:
                failures.append(f"{html_file}: canonical {e}")
                continue
            if target.resolve() != html_file.resolve():
                failures.append(
                    f"{html_file}: canonical href={href!r} resolves to {target}, "
                    f"not the file that emitted it"
                )
    return failures


def check_feed_links(root: Path, html_files: list[Path]) -> list[str]:
    failures = []
    for html_file in html_files:
        html_text = html_file.read_text(encoding="utf-8", errors="replace")
        for href in extract_hrefs(html_text, FEED_RE):
            target = href_to_path(root, html_file, href)
            if not target.is_file():
                failures.append(
                    f"{html_file}: feed link href={href!r} resolves to {target}, which does not exist"
                )
    return failures


def check_site(root: Path) -> list[str]:
    origin = read_origin(root)
    sitemaps = find_sitemaps(root)
    if not sitemaps:
        return [f"no **/sitemap.xml found under {root}"]
    html_files = find_checked_html_files(root)

    failures: list[str] = []
    failures += check_sitemap_locs_resolve(root, origin, sitemaps)
    failures += check_no_duplicate_locs(sitemaps)
    failures += check_robots_txt(root, origin, sitemaps)
    failures += check_canonical_tags(root, origin, html_files)
    failures += check_feed_links(root, html_files)
    return failures


def main() -> int:
    if len(sys.argv) != 2:
        print(f"usage: {sys.argv[0]} <staged-site-root>", file=sys.stderr)
        return 2

    root = Path(sys.argv[1])
    if not root.is_dir():
        print(f"error: {root} is not a directory", file=sys.stderr)
        return 2

    try:
        failures = check_site(root)
    except SiteCheckError as e:
        print(f"error: {e}", file=sys.stderr)
        return 1

    if failures:
        print(f"{len(failures)} problem(s) found in {root}:")
        for failure in failures:
            print(f"  - {failure}")
        return 1

    print(f"OK: staged site under {root} passed all checks.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
