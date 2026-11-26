//! LIF-524: the starting text of a project's conventions page.
//!
//! A conventions page holds the rules agents and people follow when working
//! in one project's tracker. Every new project starts with one, empty or from
//! one of these templates. The briefing prints the page every session, so each
//! template stays near 1,500 characters.

use serde::{Deserialize, Serialize};

use crate::error::LificError;

/// The title a conventions page gets when it is created with its project.
pub const PAGE_TITLE: &str = "Conventions";

/// Which text a new conventions page starts with.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Template {
    #[default]
    Empty,
    AiManaged,
    HumanManaged,
    HumanOwned,
}

impl Template {
    pub const ALL: [Template; 4] = [
        Template::Empty,
        Template::AiManaged,
        Template::HumanManaged,
        Template::HumanOwned,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Template::Empty => "empty",
            Template::AiManaged => "ai_managed",
            Template::HumanManaged => "human_managed",
            Template::HumanOwned => "human_owned",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Template::Empty => "Empty",
            Template::AiManaged => "Heavily AI managed",
            Template::HumanManaged => "Human managed",
            Template::HumanOwned => "Human owned",
        }
    }

    /// One line saying who does what, for pickers.
    pub fn summary(self) -> &'static str {
        match self {
            Template::Empty => "A blank page. Write your own rules, or leave it empty.",
            Template::AiManaged => "Agents file issues, keep them current and close them.",
            Template::HumanManaged => {
                "People write most issues; agents do the work and close them."
            }
            Template::HumanOwned => {
                "People write and close issues; agents read them closely and do the work."
            }
        }
    }

    pub fn content(self) -> &'static str {
        match self {
            Template::Empty => "",
            Template::AiManaged => include_str!("conventions/ai_managed.md"),
            Template::HumanManaged => include_str!("conventions/human_managed.md"),
            Template::HumanOwned => include_str!("conventions/human_owned.md"),
        }
    }

    pub fn parse(value: &str) -> Result<Self, LificError> {
        Self::ALL
            .into_iter()
            .find(|template| template.key() == value.trim())
            .ok_or_else(|| {
                LificError::BadRequest(format!(
                    "invalid conventions '{value}'. Use empty, ai_managed, human_managed or human_owned."
                ))
            })
    }
}

/// `GET /api/conventions/templates` row.
#[derive(Debug, Serialize)]
pub struct TemplateInfo {
    pub key: &'static str,
    pub name: &'static str,
    pub summary: &'static str,
    pub content: &'static str,
}

pub fn templates() -> Vec<TemplateInfo> {
    Template::ALL
        .into_iter()
        .map(|template| TemplateInfo {
            key: template.key(),
            name: template.name(),
            summary: template.summary(),
            content: template.content(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_template_stays_near_the_briefing_budget() {
        for template in Template::ALL {
            let chars = template.content().chars().count();
            match template {
                Template::Empty => assert_eq!(chars, 0),
                _ => assert!(
                    (1_100..=1_700).contains(&chars),
                    "{} is {chars} characters",
                    template.key()
                ),
            }
        }
    }

    #[test]
    fn templates_name_the_tracker_conventions_and_nothing_else() {
        for template in Template::ALL {
            let text = format!(
                "{} {} {}",
                template.name(),
                template.summary(),
                template.content()
            )
            .to_lowercase();
            assert!(!text.contains("agents.md"), "{}", template.key());
            assert!(!text.contains('\u{2014}') && !text.contains('\u{2013}'));
        }
    }

    #[test]
    fn parse_accepts_every_key_and_rejects_others() {
        for template in Template::ALL {
            assert_eq!(Template::parse(template.key()).unwrap(), template);
        }
        assert!(Template::parse("agents").is_err());
    }
}
