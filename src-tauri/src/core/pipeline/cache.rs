//! 任务内的答案缓存：模型原文与制图结果按内容寻址各存一份。
//!
//! 失败或停止后点「重试 / 继续」会用同一个任务重跑整条流水线：命中缓存的调用
//! 直接把上次的答案放回去，只有没跑完的部分才真的花钱。
//!
//! 缓存键是调用输入的内容哈希，所以改输入、换模型、改提示词、换母版都会自然
//! 变成另一次调用——不需要再维护一套「什么时候该让缓存失效」的判断。

use std::path::{Path, PathBuf};

use serde_json::json;
use sha2::{Digest, Sha256};

use crate::core::config::ModelProfile;
use crate::core::model::design::ImageInput;
use crate::core::net::{decode_b64, encode_b64};
use crate::error::AppResult;

/// 缓存格式版本：改动存储格式时加一，旧目录不再被读取（并随任务一起删除）。
const FORMAT: &str = "v1";

pub struct AnswerCache {
    dir: PathBuf,
}

impl AnswerCache {
    /// 缓存与任务同生命周期：放在任务目录下，任务被删除时一起清掉。
    pub fn new(app_data: &Path, job_id: &str) -> Self {
        Self {
            dir: app_data
                .join("logs")
                .join(job_id)
                .join("cache")
                .join(FORMAT),
        }
    }

    /// 上次这一步成功的原文，没有就是未命中。
    pub fn text(&self, key: &str) -> Option<String> {
        let raw = std::fs::read_to_string(self.entry(key, "json")).ok()?;
        let value: serde_json::Value = serde_json::from_str(&raw).ok()?;
        value.get("text")?.as_str().map(str::to_string)
    }

    pub fn put_text(&self, key: &str, text: &str) {
        if let Ok(body) = serde_json::to_string(&json!({ "text": text })) {
            self.write(&self.entry(key, "json"), body.as_bytes());
        }
    }

    pub fn bytes(&self, key: &str) -> Option<Vec<u8>> {
        std::fs::read(self.entry(key, "bin")).ok()
    }

    pub fn put_bytes(&self, key: &str, bytes: &[u8]) {
        self.write(&self.entry(key, "bin"), bytes);
    }

    /// 任务已经成功跑完，缓存没有被重试复用的机会，直接清掉省磁盘。
    pub fn clear(&self) {
        if let Some(parent) = self.dir.parent() {
            let _ = std::fs::remove_dir_all(parent);
        }
    }

    fn entry(&self, key: &str, extension: &str) -> PathBuf {
        self.dir.join(format!("{key}.{extension}"))
    }

    /// 写完再改名：中途被 kill 留下的半截文件不会在下次重跑时被当成答案。
    fn write(&self, path: &Path, bytes: &[u8]) {
        if std::fs::create_dir_all(&self.dir).is_err() {
            return;
        }
        let temp = path.with_extension("tmp");
        if std::fs::write(&temp, bytes).is_ok() {
            let _ = std::fs::rename(&temp, path);
        }
    }
}

/// 一次可缓存调用的身份：缓存本体加作用域（步骤名）。
///
/// 作用域进 key，所以两页内容碰巧相同的幻灯片不会互相顶掉答案，重跑时也能一眼看出
/// 某份缓存属于哪一步。
#[derive(Clone, Copy)]
pub struct StepCache<'a> {
    pub cache: &'a AnswerCache,
    pub scope: &'a str,
}

impl StepCache<'_> {
    pub fn key(&self, parts: &[&[u8]]) -> String {
        let mut all: Vec<&[u8]> = Vec::with_capacity(parts.len() + 1);
        all.push(self.scope.as_bytes());
        all.extend_from_slice(parts);
        crate::core::pipeline::cache::key_for(&all)
    }

    /// 设计调用的键：模型身份、系统与用户提示词、随请求发出去的图片。
    ///
    /// 首次生成、JSON 重解析、字段修复共用这个作用域，但提示词不同，所以各自是一次
    /// 独立缓存——重跑时连「修复那一轮」的答案也能直接回放。
    pub fn design_key(
        &self,
        profile: &ModelProfile,
        system: &str,
        prompt: &str,
        images: &[ImageInput],
    ) -> String {
        let identity = profile_identity(profile);
        let mut parts: Vec<&[u8]> = vec![
            self.scope.as_bytes(),
            identity.as_bytes(),
            system.as_bytes(),
            prompt.as_bytes(),
        ];
        let names: Vec<&str> = images.iter().map(|image| image.filename.as_str()).collect();
        let mimes: Vec<&str> = images
            .iter()
            .map(|image| image.mime_type.as_str())
            .collect();
        for name in names.iter() {
            parts.push(name.as_bytes());
        }
        for mime in mimes.iter() {
            parts.push(mime.as_bytes());
        }
        for image in images {
            parts.push(image.b64.as_bytes());
        }
        key_for(&parts)
    }

    pub fn text(&self, key: &str) -> Option<String> {
        self.cache.text(key)
    }

    pub fn put_text(&self, key: &str, text: &str) {
        self.cache.put_text(key, text);
    }
}

/// 制图调用的缓存包装：命中就回放上次那张图，未命中才真的生成。
///
/// 图片以原始字节存，命中时再编码回 base64——调用方（保存图片那一步）不用区分
/// 这张图是新生成的还是回放的。
pub async fn cached_image<F, Fut>(
    step: Option<StepCache<'_>>,
    parts: &[&[u8]],
    call: F,
) -> AppResult<String>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = AppResult<String>>,
{
    let key = step.map(|step| step.key(parts));
    if let (Some(step), Some(key)) = (step, key.as_deref()) {
        if let Some(bytes) = step.cache.bytes(key) {
            return Ok(encode_b64(&bytes));
        }
    }
    let text = call().await?;
    if let (Some(step), Some(key)) = (step, key.as_deref()) {
        // 上游直接给 URL 的协议没有可回放的字节，这种答案不缓存。
        if let Ok(bytes) = decode_b64(&text) {
            step.cache.put_bytes(key, &bytes);
        }
    }
    Ok(text)
}

/// 模型身份：换了模型或换了网关，缓存就该失效，所以两者都进键。
pub fn profile_identity(profile: &ModelProfile) -> String {
    format!(
        "{}|{}|{}|{}",
        profile.protocol,
        profile.base_url,
        profile.model,
        profile.api_version.as_deref().unwrap_or_default()
    )
}

/// 长度前缀 + 内容，避免相邻片段拼接出同一个字节序列。
fn key_for(parts: &[&[u8]]) -> String {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update((part.len() as u64).to_le_bytes());
        hasher.update(part);
    }
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cache(tag: &str) -> AnswerCache {
        let dir =
            std::env::temp_dir().join(format!("dreampaper-cache-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        AnswerCache::new(&dir, "job")
    }

    #[test]
    fn a_written_answer_comes_back_and_an_unwritten_one_does_not() {
        let cache = cache("roundtrip");
        assert!(cache.text("a").is_none());
        assert!(cache.bytes("a").is_none());
        cache.put_text("a", "第一次的答案");
        cache.put_bytes("b", &[1, 2, 3]);
        assert_eq!(cache.text("a").as_deref(), Some("第一次的答案"));
        assert_eq!(cache.bytes("b"), Some(vec![1, 2, 3]));
        cache.clear();
    }

    #[test]
    fn a_torn_write_is_not_an_answer() {
        let cache = cache("torn");
        cache.put_text("k", "完整答案");
        // 半截文件（模拟写一半被 kill）必须当未命中。
        std::fs::write(cache.dir.join("torn.json"), "{\"text\":\"截断").unwrap();
        assert!(cache.text("torn").is_none());
        assert_eq!(cache.text("k").as_deref(), Some("完整答案"));
        cache.clear();
    }

    #[test]
    fn the_key_covers_every_input_that_changes_the_answer() {
        let cache = cache("keys");
        let profile: ModelProfile = serde_json::from_value(serde_json::json!({
            "id": "design", "role": "design", "name": "Design", "protocol": "openai_chat",
            "base_url": "https://example.test/v1", "model": "Build/model", "headers": {},
            "max_retries": 0, "output_defaults": {}
        }))
        .unwrap();
        let step = StepCache {
            cache: &cache,
            scope: "ppt_page_plan_1",
        };
        let image = ImageInput {
            filename: "master.png".to_string(),
            mime_type: "image/png".to_string(),
            b64: "AAAA".to_string(),
        };
        let base = step.design_key(&profile, "system", "prompt", &[image.clone()]);

        assert_ne!(
            base,
            step.design_key(&profile, "system", "prompt 2", &[image.clone()])
        );
        assert_ne!(
            base,
            step.design_key(&profile, "另一个 system", "prompt", &[image.clone()])
        );
        assert_ne!(base, step.design_key(&profile, "system", "prompt", &[]));
        let other_image = ImageInput {
            b64: "BBBB".to_string(),
            ..image.clone()
        };
        assert_ne!(
            base,
            step.design_key(&profile, "system", "prompt", &[other_image])
        );
        // 相同作用域换个步骤名（页号）就是另一次调用，两页内容相同也不会共用答案。
        let other_scope = StepCache {
            cache: &cache,
            scope: "ppt_page_plan_2",
        };
        assert_ne!(
            base,
            other_scope.design_key(&profile, "system", "prompt", &[image.clone()])
        );
        // 换模型或换网关 = 另一次调用。
        let swapped = ModelProfile {
            model: "Build/other".to_string(),
            ..profile.clone()
        };
        assert_ne!(
            base,
            step.design_key(&swapped, "system", "prompt", &[image.clone()])
        );
        let moved = ModelProfile {
            base_url: "https://other.test/v1".to_string(),
            ..profile.clone()
        };
        assert_ne!(base, step.design_key(&moved, "system", "prompt", &[image]));
        // 同样的输入必然得到同样的键——重跑才可能命中。
        let again = step.design_key(&profile, "system", "prompt", &[]);
        assert_eq!(again, step.design_key(&profile, "system", "prompt", &[]));
        cache.clear();
    }

    #[tokio::test]
    async fn an_image_is_generated_once_and_replayed_afterwards() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let cache = cache("image");
        let step = StepCache {
            cache: &cache,
            scope: "paper_implement",
        };
        let calls = AtomicUsize::new(0);
        let bytes = b"PNG".to_vec();
        let b64 = encode_b64(&bytes);
        let generate = || async {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok::<String, crate::error::AppError>(b64.clone())
        };

        let first = cached_image(Some(step), &[b"prompt"], generate)
            .await
            .unwrap();
        assert_eq!(decode_b64(&first).unwrap(), bytes);
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        // 同样的输入第二次不再落到生成函数上。
        let second = cached_image(Some(step), &[b"prompt"], generate)
            .await
            .unwrap();
        assert_eq!(decode_b64(&second).unwrap(), bytes);
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        // 换了提示词就是另一次调用。
        cached_image(Some(step), &[b"other prompt"], generate)
            .await
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 2);

        // 没有缓存句柄时行为跟以前一样，只是不落盘。
        cached_image(None, &[b"prompt"], generate).await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        cache.clear();
    }
}
