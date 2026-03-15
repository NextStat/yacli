use std::fs;
use std::path::Path;

use crate::error::{Result, YacliError};

struct Skill {
    name: &'static str,
    content: &'static str,
}

const SKILLS: &[Skill] = &[
    Skill {
        name: "yacli-shared",
        content: include_str!("../../skills/yacli-shared/SKILL.md"),
    },
    Skill {
        name: "yacli-mail",
        content: include_str!("../../skills/yacli-mail/SKILL.md"),
    },
    Skill {
        name: "yacli-calendar",
        content: include_str!("../../skills/yacli-calendar/SKILL.md"),
    },
    Skill {
        name: "yacli-disk",
        content: include_str!("../../skills/yacli-disk/SKILL.md"),
    },
    Skill {
        name: "yacli-daily-briefing",
        content: include_str!("../../skills/yacli-daily-briefing/SKILL.md"),
    },
    Skill {
        name: "yacli-find-and-read",
        content: include_str!("../../skills/yacli-find-and-read/SKILL.md"),
    },
    Skill {
        name: "yacli-reply-with-context",
        content: include_str!("../../skills/yacli-reply-with-context/SKILL.md"),
    },
    Skill {
        name: "yacli-attachment-to-disk",
        content: include_str!("../../skills/yacli-attachment-to-disk/SKILL.md"),
    },
    Skill {
        name: "yacli-send-file-by-mail",
        content: include_str!("../../skills/yacli-send-file-by-mail/SKILL.md"),
    },
    Skill {
        name: "yacli-send-link-by-mail",
        content: include_str!("../../skills/yacli-send-link-by-mail/SKILL.md"),
    },
    Skill {
        name: "yacli-publish-file-link",
        content: include_str!("../../skills/yacli-publish-file-link/SKILL.md"),
    },
    Skill {
        name: "yacli-revoke-public-link",
        content: include_str!("../../skills/yacli-revoke-public-link/SKILL.md"),
    },
    Skill {
        name: "yacli-invite-to-calendar",
        content: include_str!("../../skills/yacli-invite-to-calendar/SKILL.md"),
    },
];

pub const SKILL_COUNT: usize = 13;

pub fn skill_names() -> Vec<&'static str> {
    SKILLS.iter().map(|s| s.name).collect()
}

pub fn skill_content(name: &str) -> Option<&'static str> {
    SKILLS
        .iter()
        .find(|skill| skill.name == name)
        .map(|skill| skill.content)
}

pub fn skill_description(name: &str) -> Option<String> {
    let content = skill_content(name)?;
    parse_frontmatter_description(content)
}

pub fn skill_prompt_name(name: &str) -> Option<&'static str> {
    match name {
        "yacli-shared" => Some("shared"),
        "yacli-mail" => Some("mail"),
        "yacli-calendar" => Some("calendar"),
        "yacli-disk" => Some("disk"),
        "yacli-daily-briefing" => Some("daily-briefing"),
        "yacli-find-and-read" => Some("find-and-read"),
        "yacli-reply-with-context" => Some("reply-with-context"),
        "yacli-attachment-to-disk" => Some("attachment-to-disk"),
        "yacli-send-file-by-mail" => Some("send-file-by-mail"),
        "yacli-send-link-by-mail" => Some("send-link-by-mail"),
        "yacli-publish-file-link" => Some("publish-file-link"),
        "yacli-revoke-public-link" => Some("revoke-public-link"),
        "yacli-invite-to-calendar" => Some("invite-to-calendar"),
        _ => None,
    }
}

pub fn prompt_skill_name(prompt_name: &str) -> Option<&'static str> {
    SKILLS.iter().find_map(|skill| {
        (skill_prompt_name(skill.name) == Some(prompt_name)).then_some(skill.name)
    })
}

/// Write all embedded skills to `target_dir/<skill-name>/SKILL.md`.
/// Returns the number of skills written.
pub fn install_skills(target_dir: &Path) -> Result<usize> {
    for skill in SKILLS {
        let skill_dir = target_dir.join(skill.name);
        fs::create_dir_all(&skill_dir).map_err(|err| {
            YacliError::Io(format!(
                "failed to create skill directory {}: {err}",
                skill_dir.display()
            ))
        })?;
        let skill_path = skill_dir.join("SKILL.md");
        fs::write(&skill_path, skill.content).map_err(|err| {
            YacliError::Io(format!(
                "failed to write skill {}: {err}",
                skill_path.display()
            ))
        })?;
    }
    Ok(SKILLS.len())
}

fn parse_frontmatter_description(content: &str) -> Option<String> {
    let frontmatter = content
        .strip_prefix("---\n")?
        .split_once("\n---\n")
        .map(|(frontmatter, _)| frontmatter)?;
    let raw = frontmatter
        .lines()
        .find_map(|line| line.strip_prefix("description:"))?
        .trim();
    Some(raw.trim_matches('"').to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn skill_count_matches_constant() {
        assert_eq!(SKILLS.len(), SKILL_COUNT);
    }

    #[test]
    fn skill_names_returns_all_names() {
        let names = skill_names();
        assert_eq!(names.len(), SKILL_COUNT);
        assert!(names.contains(&"yacli-shared"));
        assert!(names.contains(&"yacli-mail"));
        assert!(names.contains(&"yacli-calendar"));
        assert!(names.contains(&"yacli-disk"));
        assert!(names.contains(&"yacli-daily-briefing"));
        assert!(names.contains(&"yacli-find-and-read"));
        assert!(names.contains(&"yacli-reply-with-context"));
        assert!(names.contains(&"yacli-attachment-to-disk"));
        assert!(names.contains(&"yacli-send-file-by-mail"));
        assert!(names.contains(&"yacli-send-link-by-mail"));
        assert!(names.contains(&"yacli-publish-file-link"));
        assert!(names.contains(&"yacli-revoke-public-link"));
        assert!(names.contains(&"yacli-invite-to-calendar"));
    }

    #[test]
    fn skill_names_are_valid_per_spec() {
        for skill in SKILLS {
            assert!(
                skill.name.len() <= 64,
                "{}: name exceeds 64 chars",
                skill.name
            );
            assert!(
                skill
                    .name
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
                "{}: name contains invalid characters",
                skill.name
            );
            assert!(
                !skill.name.starts_with('-') && !skill.name.ends_with('-'),
                "{}: name starts or ends with hyphen",
                skill.name
            );
            assert!(
                !skill.name.contains("--"),
                "{}: name contains consecutive hyphens",
                skill.name
            );
        }
    }

    #[test]
    fn skill_content_has_valid_frontmatter() {
        for skill in SKILLS {
            assert!(
                skill.content.starts_with("---\n"),
                "{}: missing frontmatter start",
                skill.name
            );
            let after_first = &skill.content[4..];
            assert!(
                after_first.contains("\n---\n"),
                "{}: missing frontmatter end",
                skill.name
            );

            assert!(
                skill.content.contains(&format!("name: {}", skill.name)),
                "{}: frontmatter name does not match directory name",
                skill.name
            );
            assert!(
                skill.content.contains("description:"),
                "{}: missing description in frontmatter",
                skill.name
            );
        }
    }

    #[test]
    fn skill_content_within_size_limits() {
        for skill in SKILLS {
            let line_count = skill.content.lines().count();
            assert!(
                line_count <= 500,
                "{}: {} lines exceeds 500 line limit",
                skill.name,
                line_count
            );
        }
    }

    #[test]
    fn skill_description_within_spec_limit() {
        for skill in SKILLS {
            let fm_end = skill.content[4..].find("\n---\n").unwrap() + 4;
            let frontmatter = &skill.content[4..fm_end];
            let desc_line = frontmatter
                .lines()
                .find(|line| line.starts_with("description:"))
                .unwrap_or_else(|| panic!("{}: no description line", skill.name));
            let desc = desc_line.trim_start_matches("description:").trim();
            let desc = desc.trim_matches('"');
            assert!(
                desc.len() <= 1024,
                "{}: description is {} chars, exceeds 1024",
                skill.name,
                desc.len()
            );
            assert!(!desc.is_empty(), "{}: description is empty", skill.name);
        }
    }

    #[test]
    fn install_skills_writes_all_files() {
        let temp = tempdir().expect("tempdir");
        let count = install_skills(temp.path()).expect("install");
        assert_eq!(count, SKILL_COUNT);

        for skill in SKILLS {
            let path = temp.path().join(skill.name).join("SKILL.md");
            assert!(path.exists(), "{}/SKILL.md not found", skill.name);
            let content = fs::read_to_string(&path).expect("read");
            assert_eq!(content, skill.content);
        }
    }

    #[test]
    fn install_skills_is_idempotent() {
        let temp = tempdir().expect("tempdir");
        install_skills(temp.path()).expect("first install");
        let count = install_skills(temp.path()).expect("second install");
        assert_eq!(count, SKILL_COUNT);

        for skill in SKILLS {
            let path = temp.path().join(skill.name).join("SKILL.md");
            let content = fs::read_to_string(&path).expect("read");
            assert_eq!(content, skill.content);
        }
    }

    #[test]
    fn skill_content_and_description_are_available() {
        let content = skill_content("yacli-mail").expect("skill content");
        assert!(content.contains("# yacli mail"));

        let description = skill_description("yacli-mail").expect("skill description");
        assert!(description.contains("Яндекс Почта"));
    }

    #[test]
    fn prompt_and_skill_name_mapping_is_bidirectional() {
        assert_eq!(prompt_skill_name("mail"), Some("yacli-mail"));
        assert_eq!(
            prompt_skill_name("daily-briefing"),
            Some("yacli-daily-briefing")
        );
        assert_eq!(skill_prompt_name("yacli-shared"), Some("shared"));
        assert_eq!(
            skill_prompt_name("yacli-reply-with-context"),
            Some("reply-with-context")
        );
        assert_eq!(
            prompt_skill_name("attachment-to-disk"),
            Some("yacli-attachment-to-disk")
        );
        assert_eq!(
            skill_prompt_name("yacli-send-file-by-mail"),
            Some("send-file-by-mail")
        );
        assert_eq!(
            prompt_skill_name("send-link-by-mail"),
            Some("yacli-send-link-by-mail")
        );
        assert_eq!(
            prompt_skill_name("publish-file-link"),
            Some("yacli-publish-file-link")
        );
        assert_eq!(
            prompt_skill_name("revoke-public-link"),
            Some("yacli-revoke-public-link")
        );
        assert_eq!(
            prompt_skill_name("invite-to-calendar"),
            Some("yacli-invite-to-calendar")
        );
        assert_eq!(
            skill_prompt_name("yacli-publish-file-link"),
            Some("publish-file-link")
        );
        assert_eq!(
            skill_prompt_name("yacli-revoke-public-link"),
            Some("revoke-public-link")
        );
        assert_eq!(
            skill_prompt_name("yacli-invite-to-calendar"),
            Some("invite-to-calendar")
        );
    }
}
