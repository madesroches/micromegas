use micromegas::servers::mcp::skills::{Skills, parse_frontmatter};
use rmcp::model::{ResourceContents, SkillResources, sha256_digest};

#[test]
fn builtin_manifest_digest_matches_content() {
    let skills = Skills::new(None).expect("skills");
    let list = skills.skills();
    let names: Vec<_> = list.iter().map(|s| s.name().to_string()).collect();
    assert_eq!(names, ["micromegas-query", "micromegas-notebooks"]);
    for skill in list {
        let read = skills.read_resource(&skill.uri).expect("readable");
        let ResourceContents::TextResourceContents { text, .. } = &read.contents[0] else {
            panic!("expected text");
        };
        let SkillResources::Files(files) = &skill.resources else {
            panic!("expected files");
        };
        assert_eq!(files[0].digest, sha256_digest(text.as_bytes()));
        assert_eq!(files[0].size, text.len() as u64);
        skill.validate().expect("valid");
    }
}

#[test]
fn frontmatter_parsing() {
    let (n, d) =
        parse_frontmatter("---\nname: a\ndescription: b c\nother: x\n---\nbody").expect("fm");
    assert_eq!((n.as_str(), d.as_str()), ("a", "b c"));
    assert!(parse_frontmatter("no frontmatter").is_err());
}

#[test]
fn skills_dir_serves_extra_skills_and_rejects_symlink_escape() {
    let outside = tempfile::tempdir().expect("tmp");
    std::fs::write(outside.path().join("secret.md"), "secret").expect("write");
    let dir = tempfile::tempdir().expect("tmp");
    for name in ["extra-skill", "escaping-skill"] {
        let skill_dir = dir.path().join(name);
        std::fs::create_dir(&skill_dir).expect("mkdir");
        std::fs::write(
            skill_dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: x\n---\nhello"),
        )
        .expect("write");
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink(
        outside.path().join("secret.md"),
        dir.path().join("escaping-skill").join("link.md"),
    )
    .expect("symlink");
    let skills = Skills::new(Some(dir.path())).expect("skills");
    let names: Vec<_> = skills
        .skills()
        .iter()
        .map(|s| s.name().to_string())
        .collect();
    assert!(names.contains(&"extra-skill".to_string()));
    #[cfg(unix)]
    {
        assert!(!names.contains(&"escaping-skill".to_string()));
        assert!(
            skills
                .read_resource("skill://escaping-skill/link.md")
                .is_none()
        );
    }
    assert!(Skills::new(Some(&dir.path().join("missing"))).is_err());
}

#[test]
fn claude_plugin_copies_match_canonical_skills() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../claude-plugin/skills");
    if !root.exists() {
        return;
    }
    for name in ["micromegas-query", "micromegas-notebooks"] {
        let canonical = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("src/servers/mcp/skills")
                .join(name)
                .join("SKILL.md"),
        )
        .expect("canonical");
        let copy = std::fs::read_to_string(root.join(name).join("SKILL.md")).unwrap_or_else(|_| {
            panic!("{name} missing in claude-plugin; run build/sync_claude_plugin_skills.py")
        });
        assert_eq!(
            canonical, copy,
            "{name} differs; run build/sync_claude_plugin_skills.py"
        );
    }
}

#[test]
fn claude_marketplace_matches_plugin_manifest() {
    let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let plugin_dir = repo_root.join("claude-plugin");
    if !plugin_dir.exists() {
        return;
    }
    let read_json = |path: std::path::PathBuf| -> serde_json::Value {
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
        serde_json::from_str(&text)
            .unwrap_or_else(|e| panic!("cannot parse {}: {e}", path.display()))
    };
    let marketplace = read_json(repo_root.join(".claude-plugin/marketplace.json"));
    let plugin = read_json(plugin_dir.join(".claude-plugin/plugin.json"));
    let plugin_dir = plugin_dir.canonicalize().expect("plugin dir");
    let entry = marketplace["plugins"]
        .as_array()
        .expect("plugins array")
        .iter()
        .find(|e| {
            e["source"]
                .as_str()
                .and_then(|s| repo_root.join(s).canonicalize().ok())
                .is_some_and(|p| p == plugin_dir)
        })
        .expect("marketplace has no entry whose source is claude-plugin");
    assert_eq!(
        entry["name"], plugin["name"],
        "marketplace entry name must equal plugin.json name"
    );
    assert!(
        entry.get("version").is_none(),
        "keep the version only in plugin.json"
    );
}
