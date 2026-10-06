//! Stage hooks: the two seams a pipeline run exposes.
//!
//! * **Event hooks** report what a design-model step is doing: `Begin`,
//!   `Delta`, `Reset`, `End` (`crate::event::DesignLog`, delivered through a
//!   `DesignSink`). The pipeline emits them; `execute` turns them into
//!   `job://design` events and persisted rows, so the model text reaches the
//!   UI per step without the pipeline knowing about the webview.
//! * **Context hooks** inject prompt sections. A stage prompt is prompt assets
//!   plus a set of named sections; anything that is not intrinsic to the call
//!   (output contract, content inventory, retrieval evidence, master contract,
//!   advisor suggestions) is contributed by a hook registered for that stage,
//!   so a section can be added, dropped or reordered without touching the
//!   stage code.

use crate::core::prompt::{compose_prompt, PromptAsset};

/// One named block of a stage prompt.
///
/// `order` only decides where the block lands relative to the others; the
/// default is 0 and the output contract conventionally goes last.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Section {
    pub name: String,
    pub body: String,
    pub order: i32,
}

impl Section {
    pub fn new(name: impl Into<String>, body: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            body: body.into(),
            order: 0,
        }
    }

    pub fn at(mut self, order: i32) -> Self {
        self.order = order;
        self
    }
}

/// Order used for output contracts so they always close the prompt.
pub const CONTRACT_ORDER: i32 = 100;

/// Something that contributes prompt sections to the stages it cares about.
pub trait ContextHook: Send + Sync {
    /// Short identifier, reported in the stage message so the injected context
    /// is visible in the progress log.
    fn name(&self) -> &str;
    /// Sections for `stage`; empty when this hook has nothing for it.
    fn sections(&self, stage: &str) -> Vec<Section>;
}

/// A fixed section attached to one or more stages.
pub struct StaticContext {
    name: String,
    stages: Vec<String>,
    section: Section,
}

impl StaticContext {
    pub fn new(
        name: impl Into<String>,
        stages: &[&str],
        title: impl Into<String>,
        body: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            stages: stages.iter().map(|stage| (*stage).to_string()).collect(),
            section: Section::new(title, body),
        }
    }

    pub fn at(mut self, order: i32) -> Self {
        self.section.order = order;
        self
    }

    /// Mark the section as an output contract: it closes the prompt.
    pub fn contract(self) -> Self {
        self.at(CONTRACT_ORDER)
    }
}

impl ContextHook for StaticContext {
    fn name(&self) -> &str {
        &self.name
    }

    fn sections(&self, stage: &str) -> Vec<Section> {
        if self.stages.iter().any(|item| item == stage) {
            vec![self.section.clone()]
        } else {
            Vec::new()
        }
    }
}

/// The prompt a stage ends up sending, plus which hooks contributed to it.
#[derive(Debug)]
pub struct Composed {
    pub prompt: String,
    /// Hook names that injected at least one section, in registration order.
    pub injected: Vec<String>,
}

/// The context hook chain of one pipeline run.
#[derive(Default)]
pub struct ContextHooks {
    hooks: Vec<Box<dyn ContextHook>>,
}

impl ContextHooks {
    pub fn register(&mut self, hook: impl ContextHook + 'static) {
        self.hooks.push(Box::new(hook));
    }

    pub fn len(&self) -> usize {
        self.hooks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.hooks.is_empty()
    }

    /// Every section the chain contributes to `stage`, in registration order.
    pub fn sections(&self, stage: &str) -> Vec<(String, Section)> {
        self.hooks
            .iter()
            .flat_map(|hook| {
                hook.sections(stage)
                    .into_iter()
                    .map(move |section| (hook.name().to_string(), section))
            })
            .collect()
    }

    /// Assemble the stage prompt: prompt assets first, then the base sections
    /// the stage supplies itself merged with the hook sections, sorted by
    /// `order` (stable, so ties keep base-then-registration order).
    pub fn compose(&self, stage: &str, assets: &[PromptAsset], base: Vec<Section>) -> Composed {
        let mut injected: Vec<String> = Vec::new();
        let mut sections: Vec<Section> = base;
        for (name, section) in self.sections(stage) {
            if !injected.iter().any(|item| item == &name) {
                injected.push(name);
            }
            sections.push(section);
        }
        sections.sort_by_key(|section| section.order);
        let owned: Vec<(&str, String)> = sections
            .iter()
            .map(|section| (section.name.as_str(), section.body.clone()))
            .collect();
        Composed {
            prompt: compose_prompt(assets, &owned),
            injected,
        }
    }
}

/// Human-readable summary of what a composed prompt was injected with, for
/// the stage message ("注入 Output Contract / Memory Advisor").
pub fn injected_summary(composed: &Composed) -> String {
    if composed.injected.is_empty() {
        "无注入".to_string()
    } else {
        composed.injected.join(" / ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asset(key: &str, content: &str) -> PromptAsset {
        PromptAsset {
            key: key.to_string(),
            version: "v".to_string(),
            hash: "h".to_string(),
            content: content.to_string(),
        }
    }

    #[test]
    fn hooks_only_inject_into_their_stages_and_contracts_close_the_prompt() {
        let mut hooks = ContextHooks::default();
        hooks.register(
            StaticContext::new("contract", &["design"], "Output Contract", "{}").contract(),
        );
        hooks.register(StaticContext::new(
            "inventory",
            &["design", "structure"],
            "User Input",
            "title: x",
        ));
        hooks.register(StaticContext::new(
            "evidence",
            &["outline"],
            "Evidence",
            "urls",
        ));

        let design = hooks.compose(
            "design",
            &[asset("global/system.md", "sys")],
            vec![Section::new("Task", "do it")],
        );
        assert_eq!(design.injected, vec!["contract", "inventory"]);
        let prompt = &design.prompt;
        assert!(prompt.starts_with("## global/system.md\nsys"));
        assert!(prompt.find("## Task") < prompt.find("## User Input"));
        assert!(
            prompt.find("## User Input") < prompt.find("## Output Contract"),
            "契约必须收尾，而不是按注册顺序排在前面"
        );
        assert!(!prompt.contains("## Evidence"));

        let structure = hooks.compose("structure", &[], Vec::new());
        assert_eq!(structure.injected, vec!["inventory"]);
        assert!(!structure.prompt.contains("Output Contract"));
    }

    #[test]
    fn explicit_order_moves_a_section_ahead_of_the_base_sections() {
        let mut hooks = ContextHooks::default();
        hooks.register(StaticContext::new("plan", &["design"], "Structure Plan", "{}").at(-10));
        let composed = hooks.compose("design", &[], vec![Section::new("Task", "t")]);
        assert!(composed.prompt.find("## Structure Plan") < composed.prompt.find("## Task"));
        assert_eq!(injected_summary(&composed), "plan");
        assert_eq!(
            injected_summary(&hooks.compose("other", &[], Vec::new())),
            "无注入"
        );
    }
}
