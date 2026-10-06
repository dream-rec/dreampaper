//! Advisor role: compares recalled cases against the new task.
//!
//! The design model is asked, as an advisor rather than a designer, which
//! parts of the candidate cases transfer to the new brief: reusable layout,
//! terminology mapping, and failure modes to steer away from. The answer is a
//! structured section injected into the main design prompt through the
//! context-hook chain. With no candidates there is no call at all; a failed or
//! unparseable call degrades to no section, never to a failed job.

use serde_json::Value;

use crate::core::config::ModelProfile;
use crate::core::memory::{candidates_text, Fingerprint, MemoryCase};
use crate::core::net::parse_json_response;
use crate::core::pipeline::cache::{AnswerCache, StepCache};
use crate::core::pipeline::contract;
use crate::core::pipeline::hook::{ContextHook, Section};
use crate::core::pipeline::runner::{DesignCall, DesignStep};
use crate::core::prompt::{compose_prompt, PromptStore};
use crate::event::DesignSink;

pub const STEP: &str = "advisor";
pub const LABEL: &str = "比对历史案例";
pub const SECTION_TITLE: &str = "Advisor Suggestions From Similar Cases";

/// Advice that passed the shape check.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Advice {
    pub value: Value,
}

impl Advice {
    /// Accepts `{"advice": {...}}` with at least one non-empty list among the
    /// three suggestion kinds; anything thinner is treated as no advice.
    pub fn from_value(value: Value) -> Option<Self> {
        let advice = value.get("advice")?.as_object()?;
        let filled = ["reusable_layout", "term_mapping", "failure_modes_to_avoid"]
            .iter()
            .any(|key| {
                advice
                    .get(*key)
                    .and_then(Value::as_array)
                    .is_some_and(|items| !items.is_empty())
            });
        filled.then(|| Self {
            value: Value::Object(advice.clone()),
        })
    }

    pub fn section_body(&self) -> String {
        format!(
            "These suggestions come from an advisor that compared earlier similar tasks. \
             Reuse what transfers, keep the new brief as the only source of content, and \
             avoid the listed failure modes.\n{}",
            serde_json::to_string_pretty(&self.value).unwrap_or_default()
        )
    }
}

/// Injects the advice into the stages it was produced for.
pub struct AdvisorHook {
    stages: Vec<String>,
    advice: Advice,
}

impl AdvisorHook {
    pub fn new(stages: &[&str], advice: Advice) -> Self {
        Self {
            stages: stages.iter().map(|stage| (*stage).to_string()).collect(),
            advice,
        }
    }
}

impl ContextHook for AdvisorHook {
    fn name(&self) -> &str {
        "memory-advisor"
    }

    fn sections(&self, stage: &str) -> Vec<Section> {
        if self.stages.iter().any(|item| item == stage) {
            vec![Section::new(SECTION_TITLE, self.advice.section_body()).at(50)]
        } else {
            Vec::new()
        }
    }
}

pub struct AdvisorRun<'a> {
    pub prompts: &'a PromptStore,
    pub profile: &'a ModelProfile,
    pub proxy_url: Option<&'a str>,
    pub design_log: DesignSink<'a>,
    /// 有缓存时这次比对只买一次，重跑任务命中就回放。
    pub cache: Option<&'a AnswerCache>,
}

impl AdvisorRun<'_> {
    /// One model call at most. `None` on no candidates, on transport failure,
    /// and on an answer that does not carry usable advice.
    pub async fn advise(&self, task: &Fingerprint, cases: &[MemoryCase]) -> Option<Advice> {
        if cases.is_empty() {
            return None;
        }
        let assets = self
            .prompts
            .load_all(&["global/system.md", "roles/advisor.md"])
            .ok()?;
        let mode_label = if task.mode == "ppt_slide" {
            "academic slide deck"
        } else {
            "academic paper figure"
        };
        let prompt = compose_prompt(
            &assets,
            &[
                (
                    "New Task",
                    format!(
                        "mode: {mode_label}\ntitle: {}\nbrief: {}",
                        task.title,
                        task.brief.chars().take(2400).collect::<String>()
                    ),
                ),
                ("Candidate Cases", candidates_text(cases)),
                ("Output Contract", contract::advisor().to_string()),
            ],
        );
        let call = DesignCall {
            profile: self.profile,
            system_prompt: &assets[0].content,
            user_prompt: &prompt,
            images: &[],
            proxy_url: self.proxy_url,
            response_sink: None,
            log: Some(DesignStep {
                sink: self.design_log,
                step: STEP,
                label: LABEL,
            }),
            cache: self.cache.map(|cache| StepCache { cache, scope: STEP }),
        };
        let text = call.first().await.ok()?;
        let parsed = parse_json_response(&text).ok()?;
        Advice::from_value(parsed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::pipeline::hook::ContextHooks;
    use serde_json::json;

    #[test]
    fn thin_or_malformed_advice_is_dropped() {
        assert!(Advice::from_value(json!({"advice": {}})).is_none());
        assert!(Advice::from_value(json!({"advice": {"reusable_layout": []}})).is_none());
        assert!(Advice::from_value(json!({"nope": 1})).is_none());
        assert!(Advice::from_value(json!("text")).is_none());
        let advice = Advice::from_value(json!({"advice": {
            "reusable_layout": ["three-lane pipeline"],
            "term_mapping": [],
            "failure_modes_to_avoid": []
        }}))
        .unwrap();
        assert_eq!(
            advice.value["reusable_layout"][0],
            json!("three-lane pipeline")
        );
    }

    #[test]
    fn hook_injects_only_into_its_stages_ahead_of_the_contract() {
        let advice = Advice::from_value(json!({"advice": {
            "reusable_layout": ["keep the dashed feedback band"],
            "term_mapping": [{"from": "重排", "to": "交叉编码器重排"}],
            "failure_modes_to_avoid": ["collapsing OCR + layout into one box"]
        }}))
        .unwrap();
        let mut hooks = ContextHooks::default();
        hooks.register(AdvisorHook::new(&["paper_design"], advice));
        hooks.register(
            crate::core::pipeline::hook::StaticContext::new(
                "contract",
                &["paper_design"],
                "Output Contract",
                "{}",
            )
            .contract(),
        );
        let composed = hooks.compose("paper_design", &[], Vec::new());
        assert_eq!(composed.injected, vec!["memory-advisor", "contract"]);
        assert!(composed.prompt.contains(SECTION_TITLE));
        assert!(composed.prompt.contains("dashed feedback band"));
        assert!(composed.prompt.find(SECTION_TITLE) < composed.prompt.find("## Output Contract"));
        assert!(hooks
            .compose("paper_structure", &[], Vec::new())
            .injected
            .is_empty());
    }
}
