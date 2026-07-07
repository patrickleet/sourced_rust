//! Embedded agent skills: markdown guidance documents for coding agents on how
//! to use the Distributed framework, compiled into the binary from
//! `distributed_cli/skills/` via `include_str!`. Pure — [`generate_skills`]
//! returns a [`GeneratedProject`]; the `cli` module owns all filesystem writes.
//!
//! The skill *format* is the portable Agent Skills convention — a folder per
//! skill with a `SKILL.md` whose frontmatter has `name` and `description` —
//! used identically by Claude Code, OpenAI Codex, Grok Build, Gemini CLI, and
//! Pi. Discovery *locations* differ, so [`SkillsInitSpec`] carries two adapter
//! switches: `.claude/skills/` copies (Claude Code) and `.agents/skills/`
//! copies plus a managed `AGENTS.md` block (everything else).

use crate::{GeneratedFile, GeneratedProject};

/// One file of an embedded skill, addressed relative to the skill's folder.
pub struct EmbeddedFile {
    /// Path relative to `skills/<name>/` (forward slashes), e.g. `SKILL.md`.
    pub relative_path: &'static str,
    /// The embedded file contents.
    pub contents: &'static str,
}

/// A skill embedded in the binary at compile time.
pub struct EmbeddedSkill {
    /// The skill name; must equal its folder name and its frontmatter `name`.
    pub name: &'static str,
    /// The frontmatter `description`, duplicated here so `skills list` needs no
    /// runtime parsing. A unit test asserts the two stay identical.
    pub description: &'static str,
    /// The skill's files. Every skill has a `SKILL.md`; extras are supported.
    pub files: &'static [EmbeddedFile],
}

static SKILLS: [EmbeddedSkill; 3] = [
    EmbeddedSkill {
        name: "distributed-usage",
        description: "Build services with the Distributed CQRS/event-sourcing framework - aggregates, command/event handlers, read models, the outbox, and the distributed_manifest() entrypoint. Use when writing or modifying a Distributed (Rust) service.",
        files: &[EmbeddedFile {
            relative_path: "SKILL.md",
            contents: include_str!("../skills/distributed-usage/SKILL.md"),
        }],
    },
    EmbeddedSkill {
        name: "distributed-ci",
        description: "Set up CI, release workflows, and GitOps promotion for a Distributed service with dctl scaffold flags (--github, --gitops, --gitops-promote). Use when configuring pipelines, previews, releases, or deploy automation.",
        files: &[EmbeddedFile {
            relative_path: "SKILL.md",
            contents: include_str!("../skills/distributed-ci/SKILL.md"),
        }],
    },
    EmbeddedSkill {
        name: "distributed-schema",
        description: "Inspect a Distributed service manifest and render schema artifacts - dctl describe (manifest JSON), dctl schema (migration SQL or an Atlas Operator resource), and the distributed_manifest() envelope contract. Use when working on read-model schemas, migrations, or schema automation.",
        files: &[EmbeddedFile {
            relative_path: "SKILL.md",
            contents: include_str!("../skills/distributed-schema/SKILL.md"),
        }],
    },
];

/// The registry of skills embedded in this binary.
pub fn embedded_skills() -> &'static [EmbeddedSkill] {
    &SKILLS
}

/// The `AGENTS.md` path (relative to the anchor) emitted by [`generate_skills`]
/// when the agents adapter is wired. The writer treats this file specially: its
/// contents are a merge of the on-disk file, so "differs" means "update the
/// managed block", never "skip".
pub const AGENTS_MD_FILE: &str = "AGENTS.md";

const AGENTS_MD_BEGIN: &str =
    "<!-- distributed:skills:begin (managed by dctl skills init; do not edit inside) -->";
const AGENTS_MD_END: &str = "<!-- distributed:skills:end -->";

/// What to generate. The pure input to [`generate_skills`]. All paths in the
/// output are relative to the *anchor* — the parent of the skills container —
/// which is where `.claude/`, `.agents/`, and `AGENTS.md` belong.
pub struct SkillsInitSpec {
    /// The skills container directory relative to the anchor (its final path
    /// component), e.g. `.distributed`. Canonical files land under
    /// `<container>/skills/`.
    pub container: String,
    /// Copy each skill to `.claude/skills/<name>/` (Claude Code discovery).
    pub wire_claude: bool,
    /// Copy each skill to `.agents/skills/<name>/` (Codex, Grok, Gemini, Pi)
    /// and maintain the managed block in `AGENTS.md`.
    pub wire_agents: bool,
    /// The current on-disk `AGENTS.md` contents, when one exists. Only read
    /// when `wire_agents` is set; the managed block is merged into it.
    pub agents_md: Option<String>,
}

/// Generate every file `skills init` should write: the canonical
/// `<container>/skills/` tree plus the wired harness copies and merged
/// `AGENTS.md`. No I/O — the caller owns writes and per-file drift decisions.
pub fn generate_skills(spec: &SkillsInitSpec) -> GeneratedProject {
    let mut project = GeneratedProject::default();

    for skill in embedded_skills() {
        for file in skill.files {
            let mut roots = vec![format!("{}/skills", spec.container)];
            if spec.wire_claude {
                roots.push(".claude/skills".to_string());
            }
            if spec.wire_agents {
                roots.push(".agents/skills".to_string());
            }
            for root in roots {
                project.files.push(GeneratedFile {
                    path: format!("{root}/{}/{}", skill.name, file.relative_path),
                    contents: file.contents.to_string(),
                    mode: None,
                });
            }
        }
    }

    if spec.wire_agents {
        match merge_agents_md(spec.agents_md.as_deref(), &spec.container) {
            Ok(contents) => project.files.push(GeneratedFile {
                path: AGENTS_MD_FILE.to_string(),
                contents,
                mode: None,
            }),
            Err(warning) => project.warnings.push(warning),
        }
    }

    project
}

/// The sentinel-delimited managed block advertising the skills. It serves the
/// 30+ AGENTS.md-only tools that have no skills concept, alongside the
/// harnesses that discover `.agents/skills/` natively.
fn managed_block(container: &str) -> String {
    let mut block = format!(
        "{AGENTS_MD_BEGIN}\n\
         ## Distributed framework skills\n\
         \n\
         Skills for building services with Distributed live in `.agents/skills/`\n\
         (canonical copy: `{container}/skills/`). Consult the relevant skill before\n\
         scaffolding, CI, or schema work:\n\
         \n"
    );
    for skill in embedded_skills() {
        block.push_str(&format!("- **{}** — {}\n", skill.name, skill.description));
    }
    block.push_str(AGENTS_MD_END);
    block
}

/// Merge the managed block into `existing` `AGENTS.md` contents: replace an
/// existing block in place, append when absent, create when there is no file.
/// User content outside the sentinels is preserved byte-for-byte. Mismatched
/// sentinels are ambiguous — refuse with a warning rather than guess at a
/// boundary and eat user content.
fn merge_agents_md(existing: Option<&str>, container: &str) -> Result<String, String> {
    let block = managed_block(container);
    let Some(existing) = existing else {
        return Ok(format!("{block}\n"));
    };

    match (existing.find(AGENTS_MD_BEGIN), existing.find(AGENTS_MD_END)) {
        (Some(begin), Some(end)) if end >= begin => {
            let after = &existing[end + AGENTS_MD_END.len()..];
            Ok(format!("{}{block}{after}", &existing[..begin]))
        }
        (None, None) => {
            if existing.trim().is_empty() {
                Ok(format!("{block}\n"))
            } else {
                Ok(format!("{}\n\n{block}\n", existing.trim_end_matches('\n')))
            }
        }
        _ => Err(format!(
            "AGENTS.md has mismatched managed-block markers ({AGENTS_MD_BEGIN} / {AGENTS_MD_END}); \
             fix or remove them and re-run to update the Distributed skills section"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::fs;
    use std::path::Path;

    /// Parse `name:` and `description:` out of a `---`-delimited frontmatter
    /// header. Test-only: at runtime the registry carries both values.
    fn parse_frontmatter(contents: &str) -> (String, String) {
        let mut lines = contents.lines();
        assert_eq!(
            lines.next(),
            Some("---"),
            "skill must start with frontmatter"
        );
        let mut name = None;
        let mut description = None;
        for line in lines {
            if line == "---" {
                break;
            }
            if let Some(value) = line.strip_prefix("name:") {
                name = Some(value.trim().to_string());
            } else if let Some(value) = line.strip_prefix("description:") {
                description = Some(value.trim().to_string());
            }
        }
        (
            name.expect("frontmatter has name"),
            description.expect("frontmatter has description"),
        )
    }

    fn skill_md(skill: &EmbeddedSkill) -> &'static str {
        skill
            .files
            .iter()
            .find(|file| file.relative_path == "SKILL.md")
            .unwrap_or_else(|| panic!("skill {} has no SKILL.md", skill.name))
            .contents
    }

    fn spec(
        container: &str,
        claude: bool,
        agents: bool,
        agents_md: Option<&str>,
    ) -> SkillsInitSpec {
        SkillsInitSpec {
            container: container.to_string(),
            wire_claude: claude,
            wire_agents: agents,
            agents_md: agents_md.map(str::to_string),
        }
    }

    #[test]
    fn registry_matches_frontmatter() {
        for skill in embedded_skills() {
            let (name, description) = parse_frontmatter(skill_md(skill));
            assert_eq!(name, skill.name, "frontmatter name mismatch");
            assert_eq!(
                description, skill.description,
                "registry description for {} must equal its frontmatter",
                skill.name
            );
            assert!(!description.is_empty());
        }
    }

    #[test]
    fn skill_names_are_cross_harness_valid() {
        for skill in embedded_skills() {
            assert!(skill.name.len() <= 64, "{} exceeds 64 chars", skill.name);
            assert!(
                skill
                    .name
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
                "{} must be lowercase a-z0-9-",
                skill.name
            );
        }
    }

    /// Guards "added/edited a skill file, forgot the `include_str!`" drift in
    /// both directions: the registry covers exactly the on-disk files under
    /// `distributed_cli/skills/`, with identical contents.
    #[test]
    fn registry_matches_skills_directory() {
        let skills_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("skills");

        let on_disk: BTreeSet<String> = fs::read_dir(&skills_dir)
            .expect("distributed_cli/skills exists")
            .map(|entry| entry.unwrap())
            .filter(|entry| entry.path().is_dir())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        let registered: BTreeSet<String> = embedded_skills()
            .iter()
            .map(|skill| skill.name.to_string())
            .collect();
        assert_eq!(
            on_disk, registered,
            "skills/ folders must match the registry"
        );

        for skill in embedded_skills() {
            let dir = skills_dir.join(skill.name);
            let mut files = Vec::new();
            collect_files(&dir, &dir, &mut files);
            files.sort();
            let mut registered: Vec<String> = skill
                .files
                .iter()
                .map(|file| file.relative_path.to_string())
                .collect();
            registered.sort();
            assert_eq!(
                files, registered,
                "files of skill {} must match",
                skill.name
            );

            for file in skill.files {
                let disk = fs::read_to_string(dir.join(file.relative_path)).unwrap();
                assert_eq!(
                    disk, file.contents,
                    "embedded contents of {} stale",
                    skill.name
                );
            }
        }
    }

    fn collect_files(root: &Path, dir: &Path, out: &mut Vec<String>) {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                collect_files(root, &path, out);
            } else {
                out.push(
                    path.strip_prefix(root)
                        .unwrap()
                        .to_string_lossy()
                        .replace('\\', "/"),
                );
            }
        }
    }

    #[test]
    fn generate_canonical_only() {
        let project = generate_skills(&spec(".distributed", false, false, None));
        assert!(project.warnings.is_empty());
        let paths: Vec<&str> = project.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths.len(), embedded_skills().len());
        for skill in embedded_skills() {
            let expected = format!(".distributed/skills/{}/SKILL.md", skill.name);
            assert!(paths.contains(&expected.as_str()), "missing {expected}");
        }
        assert!(paths.iter().all(|p| p.starts_with(".distributed/")));
    }

    #[test]
    fn generate_wires_both_adapters() {
        let project = generate_skills(&spec(".distributed", true, true, None));
        let paths: BTreeSet<&str> = project.files.iter().map(|f| f.path.as_str()).collect();
        for skill in embedded_skills() {
            for root in [".distributed/skills", ".claude/skills", ".agents/skills"] {
                let expected = format!("{root}/{}/SKILL.md", skill.name);
                assert!(paths.contains(expected.as_str()), "missing {expected}");
            }
        }
        assert!(paths.contains(AGENTS_MD_FILE));
        // Nothing but skill folders under the harness roots (no root-level .md
        // strays — Pi historically treated those as skills).
        assert!(paths
            .iter()
            .filter(|p| p.starts_with(".agents/skills/") || p.starts_with(".claude/skills/"))
            .all(|p| p.splitn(4, '/').count() == 4));
    }

    #[test]
    fn agents_md_created_when_absent() {
        let project = generate_skills(&spec(".distributed", false, true, None));
        let agents_md = project
            .files
            .iter()
            .find(|f| f.path == AGENTS_MD_FILE)
            .expect("AGENTS.md generated");
        assert!(agents_md.contents.starts_with(AGENTS_MD_BEGIN));
        assert!(agents_md.contents.trim_end().ends_with(AGENTS_MD_END));
        for skill in embedded_skills() {
            assert!(agents_md.contents.contains(skill.name));
        }
        assert!(agents_md.contents.contains(".distributed/skills/"));
    }

    #[test]
    fn agents_md_block_appended_after_existing_content() {
        let existing = "# My project\n\nHouse rules.\n";
        let merged = merge_agents_md(Some(existing), ".distributed").unwrap();
        assert!(merged.starts_with("# My project\n\nHouse rules.\n\n"));
        assert!(merged.contains(AGENTS_MD_BEGIN));
    }

    #[test]
    fn agents_md_block_replaced_in_place() {
        let before = "# Intro\n\n";
        let after = "\n\n## Trailing user section\ndo not touch\n";
        let existing = format!("{before}{AGENTS_MD_BEGIN}\nstale contents\n{AGENTS_MD_END}{after}");
        let merged = merge_agents_md(Some(&existing), ".distributed").unwrap();
        assert!(merged.starts_with(before), "content before block preserved");
        assert!(merged.ends_with(after), "content after block preserved");
        assert!(!merged.contains("stale contents"));
        assert_eq!(merged.matches(AGENTS_MD_BEGIN).count(), 1);
        // Idempotent: merging the merged output changes nothing.
        assert_eq!(
            merge_agents_md(Some(&merged), ".distributed").unwrap(),
            merged
        );
    }

    #[test]
    fn agents_md_mismatched_sentinels_warn_instead_of_writing() {
        let existing = format!("intro\n{AGENTS_MD_BEGIN}\nno end marker\n");
        let project = generate_skills(&spec(".distributed", false, true, Some(&existing)));
        assert!(project.files.iter().all(|f| f.path != AGENTS_MD_FILE));
        assert_eq!(project.warnings.len(), 1);
        assert!(project.warnings[0].contains("mismatched"));
    }
}
