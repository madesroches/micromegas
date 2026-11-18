use anyhow::{Context, Result};
use rmcp::handler::server::skill_catalog::SkillCatalog;
use rmcp::model::{
    GetSkillResult, ListSkillsResult, ReadResourceResult, Resource, ResourceContents, Skill,
    SkillFrontmatter, SkillResource, SkillResources,
};
use std::path::Path;

const MARKDOWN: &str = "text/markdown";

const BUILTIN: &[&str] = &[
    include_str!("skills/micromegas-query/SKILL.md"),
    include_str!("skills/micromegas-notebooks/SKILL.md"),
];

struct BuiltinSkill {
    skill: Skill,
    text: &'static str,
}

/// The skills served over the MCP skills extension: the built-in ones plus those found in the
/// optional skills directory.
pub struct Skills {
    builtin: Vec<BuiltinSkill>,
    catalog: Option<SkillCatalog>,
}

/// Reads `name` and `description` from the `---` delimited frontmatter of a `SKILL.md`.
pub fn parse_frontmatter(text: &str) -> Result<(String, String)> {
    let mut lines = text.lines();
    anyhow::ensure!(lines.next() == Some("---"), "missing frontmatter");
    let (mut name, mut description) = (None, None);
    for line in lines {
        if line == "---" {
            break;
        }
        if let Some(v) = line.strip_prefix("name:") {
            name = Some(v.trim().to_string());
        } else if let Some(v) = line.strip_prefix("description:") {
            description = Some(v.trim().to_string());
        }
    }
    Ok((
        name.context("frontmatter has no name")?,
        description.context("frontmatter has no description")?,
    ))
}

fn skill_uri(name: &str) -> String {
    format!("skill://{name}/SKILL.md")
}

impl Skills {
    /// Computes the digest manifest of the built-in skills and loads `skills_dir` once.
    pub fn new(skills_dir: Option<&Path>) -> Result<Self> {
        let mut builtin = vec![];
        for text in BUILTIN {
            let (name, description) = parse_frontmatter(text)?;
            let uri = skill_uri(&name);
            let skill = Skill::new(
                uri.clone(),
                SkillFrontmatter::new(name, description),
                SkillResources::Files(vec![SkillResource::from_bytes(uri, text.as_bytes())]),
            );
            skill.validate().context("invalid built-in skill")?;
            builtin.push(BuiltinSkill { skill, text });
        }
        let catalog = match skills_dir {
            Some(dir) => Some(
                SkillCatalog::from_dir(dir)
                    .with_context(|| format!("loading skills from {}", dir.display()))?,
            ),
            None => None,
        };
        Ok(Self { builtin, catalog })
    }

    fn is_builtin_uri(&self, uri: &str) -> bool {
        self.builtin.iter().any(|b| b.skill.uri == uri)
    }

    /// Every skill; a directory skill never shadows a built-in one.
    pub fn skills(&self) -> Vec<Skill> {
        let mut skills: Vec<Skill> = self.builtin.iter().map(|b| b.skill.clone()).collect();
        if let Some(catalog) = &self.catalog {
            skills.extend(
                catalog
                    .skills()
                    .into_iter()
                    .filter(|s| !self.is_builtin_uri(&s.uri)),
            );
        }
        skills
    }

    pub fn list_skills(&self) -> ListSkillsResult {
        ListSkillsResult::with_all_items(self.skills())
    }

    pub fn get_skill(&self, uri: &str) -> Option<GetSkillResult> {
        self.skills()
            .into_iter()
            .find(|s| s.uri == uri)
            .map(GetSkillResult::new)
    }

    pub fn resources(&self) -> Vec<Resource> {
        let mut resources: Vec<Resource> = self
            .builtin
            .iter()
            .map(|b| {
                Resource::new(&b.skill.uri, b.skill.name())
                    .with_description(&b.skill.frontmatter.description)
                    .with_mime_type(MARKDOWN)
                    .with_size(b.text.len() as u64)
            })
            .collect();
        if let Some(catalog) = &self.catalog {
            resources.extend(
                catalog
                    .resources()
                    .into_iter()
                    .filter(|r| !self.is_builtin_uri(&r.uri)),
            );
        }
        resources
    }

    pub fn read_resource(&self, uri: &str) -> Option<ReadResourceResult> {
        if let Some(b) = self.builtin.iter().find(|b| b.skill.uri == uri) {
            return Some(ReadResourceResult::new(vec![
                ResourceContents::text(b.text, uri).with_mime_type(MARKDOWN),
            ]));
        }
        self.catalog.as_ref()?.read_resource(uri)
    }
}
