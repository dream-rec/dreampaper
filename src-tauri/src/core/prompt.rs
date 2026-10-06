use std::path::{Path, PathBuf};

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::error::{AppError, AppResult};

const EMBEDDED_PROMPTS: &[(&str, &str)] = &[
    (
        "global/system.md",
        include_str!("../../../prompts/global/system.md"),
    ),
    (
        "global/figure_style.md",
        include_str!("../../../prompts/global/figure_style.md"),
    ),
    (
        "global/expression.md",
        include_str!("../../../prompts/global/expression.md"),
    ),
    (
        "global/visual_subjects.md",
        include_str!("../../../prompts/global/visual_subjects.md"),
    ),
    (
        "modes/paper_figure/structure.md",
        include_str!("../../../prompts/modes/paper_figure/structure.md"),
    ),
    (
        "modes/paper_figure/design.md",
        include_str!("../../../prompts/modes/paper_figure/design.md"),
    ),
    (
        "modes/paper_figure/diagram_rules.md",
        include_str!("../../../prompts/modes/paper_figure/diagram_rules.md"),
    ),
    (
        "modes/paper_figure/plot_rules.md",
        include_str!("../../../prompts/modes/paper_figure/plot_rules.md"),
    ),
    (
        "modes/paper_figure/validator.md",
        include_str!("../../../prompts/modes/paper_figure/validator.md"),
    ),
    (
        "modes/ppt_slide/analyzer.md",
        include_str!("../../../prompts/modes/ppt_slide/analyzer.md"),
    ),
    (
        "modes/ppt_slide/design.md",
        include_str!("../../../prompts/modes/ppt_slide/design.md"),
    ),
    (
        "modes/ppt_slide/master_rules.md",
        include_str!("../../../prompts/modes/ppt_slide/master_rules.md"),
    ),
    (
        "styles/academic_ppt.md",
        include_str!("../../../prompts/styles/academic_ppt.md"),
    ),
    (
        "roles/advisor.md",
        include_str!("../../../prompts/roles/advisor.md"),
    ),
];

#[derive(Clone, Debug)]
pub struct PromptStore {
    root: PathBuf,
}

#[derive(Clone, Debug, Serialize)]
pub struct PromptAsset {
    pub key: String,
    pub version: String,
    pub hash: String,
    pub content: String,
}

impl PromptStore {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn load(&self, key: &str) -> AppResult<PromptAsset> {
        let path = self.root.join(Path::new(key));
        let content = match std::fs::read_to_string(&path) {
            Ok(content) => content,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                embedded_prompt(key).map(str::to_string).ok_or_else(|| {
                    AppError::new(
                        "prompt_not_found",
                        format!("Prompt resource not found: {}", path.display()),
                    )
                })?
            }
            Err(error) => {
                return Err(AppError::new(
                    "prompt_read_failed",
                    format!("Failed to read prompt resource {}: {error}", path.display()),
                ))
            }
        };
        let hash = short_hash(&content);
        Ok(PromptAsset {
            key: key.to_string(),
            version: hash.clone(),
            hash,
            content,
        })
    }
    pub fn load_all(&self, keys: &[&str]) -> AppResult<Vec<PromptAsset>> {
        keys.iter().map(|key| self.load(key)).collect()
    }
}

fn embedded_prompt(key: &str) -> Option<&'static str> {
    EMBEDDED_PROMPTS
        .iter()
        .find_map(|(name, content)| (*name == key).then_some(*content))
}

pub fn compose_prompt(assets: &[PromptAsset], sections: &[(&str, String)]) -> String {
    let mut parts: Vec<String> = assets
        .iter()
        .map(|asset| format!("## {}\n{}", asset.key, asset.content.trim()))
        .collect();
    parts.extend(
        sections
            .iter()
            .map(|(name, value)| format!("## {name}\n{value}")),
    );
    parts.join("\n\n")
}

fn short_hash(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    let digest = hasher.finalize();
    format!("{digest:x}").chars().take(16).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prompt_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("src-tauri has a parent")
            .join("prompts")
    }

    #[test]
    fn prompt_assets_load_and_compose() {
        let store = PromptStore::new(prompt_root());
        let keys = [
            "modes/paper_figure/diagram_rules.md",
            "modes/paper_figure/plot_rules.md",
            "modes/ppt_slide/master_rules.md",
        ];
        let assets = store.load_all(&keys).expect("prompt assets should load");
        let prompt = compose_prompt(&assets, &[("Output Contract", "{}".to_string())]);

        assert!(prompt.contains("PaperBanana"));
        assert!(prompt.contains("master_style_spec"));
        assert_eq!(
            assets
                .iter()
                .map(|asset| asset.key.as_str())
                .collect::<Vec<_>>(),
            keys
        );
        assert!(assets
            .iter()
            .all(|asset| !asset.hash.is_empty() && asset.version == asset.hash));
        assert!(
            prompt.find("## modes/paper_figure/diagram_rules.md")
                < prompt.find("## Output Contract")
        );
    }

    #[test]
    fn missing_prompt_asset_is_an_error() {
        let store = PromptStore::new(prompt_root());
        assert!(store.load("modes/does_not_exist.md").is_err());
    }

    #[test]
    fn packaged_prompts_work_without_an_external_resource_directory() {
        let store = PromptStore::new(prompt_root().join("missing-resource-directory"));
        let asset = store
            .load("global/system.md")
            .expect("packaged prompt should be embedded in the executable");

        assert!(asset.content.contains("dreampaper design agent"));
    }
}
