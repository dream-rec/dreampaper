use std::path::{Path, PathBuf};

use chrono::Utc;
use rusqlite::params;
use serde::Serialize;
use serde_json::Value;
use uuid::Uuid;

use crate::error::{AppError, AppResult};

use super::asset::{guess_mime, protocol_url, sanitize_filename};

/// 列表和选择网格只展示缩略图。原图经常是生成分辨率，整张解码进 WebKit
/// 会占一块显存；制图流程刷新界面时这块纹理被反复上传，GPU 占用就忽高忽低。
/// 送给模型的仍是磁盘上的原图，不走这个 URL。
const LIST_THUMB_WIDTH: u32 = 400;

fn list_image_url(id: &str) -> String {
    format!("{}?w={LIST_THUMB_WIDTH}", protocol_url("dp-template", id))
}
use super::store::Store;

#[derive(Clone, Debug, Serialize)]
pub struct TemplateSummary {
    pub id: String,
    pub source_id: String,
    pub kind: String,
    pub category: Option<String>,
    pub rounded_ratio: Option<String>,
    pub visual_intent: String,
    pub content_summary: String,
    pub image_url: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct TemplatePackSummary {
    pub id: String,
    pub name: String,
    pub version: Option<String>,
    pub template_count: i64,
}

#[derive(Clone, Debug)]
pub struct TemplateFile {
    pub id: String,
    pub mime_type: String,
    pub path: PathBuf,
}

#[derive(Clone, Debug)]
pub struct TemplateDetail {
    pub id: String,
    pub source_id: String,
    pub kind: String,
    pub category: Option<String>,
    pub rounded_ratio: Option<String>,
    pub visual_intent: String,
    pub content_summary: String,
    pub mime_type: String,
    pub image_path: PathBuf,
}

impl TemplateDetail {
    pub fn metadata(&self) -> Value {
        serde_json::json!({
            "id": self.id,
            "kind": self.kind,
            "category": self.category,
            "visual_intent": self.visual_intent,
            "content": self.content_summary,
            "rounded_ratio": self.rounded_ratio,
            "path_to_gt_image": self.source_id,
        })
    }
}

pub struct TemplateService<'a> {
    store: &'a Store,
    app_data: &'a Path,
}

impl<'a> TemplateService<'a> {
    pub fn new(store: &'a Store, app_data: &'a Path) -> Self {
        Self { store, app_data }
    }

    pub fn list_templates(&self, kind: String, query: String) -> AppResult<Vec<TemplateSummary>> {
        let conn = self.store.connection()?;
        let kind = kind.trim().to_string();
        let all_kinds = kind.is_empty() || kind == "all";
        let query = query.trim().to_lowercase();
        let like = format!("%{query}%");

        let mut stmt = conn.prepare(
            "SELECT id, source_id, kind, category, rounded_ratio, visual_intent, content_summary \
             FROM templates \
             WHERE (?1 = 1 OR kind = ?2) \
               AND (?3 = 1 \
                    OR lower(visual_intent) LIKE ?4 \
                    OR lower(content_summary) LIKE ?4 \
                    OR lower(coalesce(category, '')) LIKE ?4) \
             ORDER BY created_at DESC",
        )?;
        let rows = stmt.query_map(
            params![all_kinds as i64, kind, query.is_empty() as i64, like],
            |row| {
                let id: String = row.get(0)?;
                Ok(TemplateSummary {
                    image_url: list_image_url(&id),
                    id,
                    source_id: row.get(1)?,
                    kind: row.get(2)?,
                    category: row.get(3)?,
                    rounded_ratio: row.get(4)?,
                    visual_intent: row.get(5)?,
                    content_summary: row.get(6)?,
                })
            },
        )?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn import_template_image(
        &self,
        filename: String,
        mime_type: String,
        bytes: Vec<u8>,
        kind: String,
        category: Option<String>,
        visual_intent: Option<String>,
        content_summary: Option<String>,
    ) -> AppResult<TemplateSummary> {
        let kind = normalize_kind(&kind);
        let id = Uuid::new_v4().to_string();
        let safe_name = sanitize_filename(&filename);
        let suffix = Path::new(&safe_name)
            .extension()
            .and_then(|value| value.to_str())
            .map(|value| format!(".{value}"))
            .unwrap_or_else(|| ".png".to_string());
        let dir = self.app_data.join("templates").join(&id);
        std::fs::create_dir_all(&dir)?;
        let image_path = dir.join(format!("image{suffix}"));
        std::fs::write(&image_path, bytes)?;
        let detected_mime =
            if mime_type.trim().is_empty() || mime_type == "application/octet-stream" {
                guess_mime(&image_path)
            } else {
                mime_type
            };
        if !detected_mime.starts_with("image/") {
            return Err(AppError::new(
                "template_mime_unsupported",
                "Template image must be png, jpeg, webp, gif, or svg",
            ));
        }
        let visual_intent = visual_intent
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| format!("User imported {kind} visual reference: {filename}"));
        let content_summary = content_summary
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "Imported single template image. Use it as a style/layout reference, not as a background to copy.".to_string());
        let now = Utc::now().to_rfc3339();
        let conn = self.store.connection()?;
        conn.execute(
            "INSERT INTO templates(id, pack_id, source_id, kind, category, rounded_ratio, visual_intent, content_summary, image_path, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                id,
                Option::<String>::None,
                filename,
                kind,
                category,
                Option::<String>::None,
                visual_intent,
                content_summary,
                image_path.to_string_lossy().to_string(),
                now
            ],
        )?;
        Ok(TemplateSummary {
            image_url: list_image_url(&id),
            id,
            source_id: filename,
            kind,
            category,
            rounded_ratio: None,
            visual_intent,
            content_summary,
        })
    }

    pub fn import_template_pack(&self, path: String) -> AppResult<TemplatePackSummary> {
        let source = Path::new(&path);
        if !source.exists() {
            return Err(AppError::new(
                "template_pack_not_found",
                "Template pack path does not exist",
            ));
        }
        if !source.is_dir() {
            return Err(AppError::new(
                "template_pack_unsupported",
                "Desktop template pack import expects an extracted directory path",
            ));
        }
        let id = Uuid::new_v4().to_string();
        let name = source
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("template_pack")
            .to_string();
        let target_dir = self.app_data.join("templates").join(&id);
        std::fs::create_dir_all(&target_dir)?;
        let conn = self.store.connection()?;
        conn.execute(
            "INSERT INTO template_packs(id, name, version, source_path, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, name, Option::<String>::None, path, Utc::now().to_rfc3339()],
        )?;
        let mut count = 0_i64;
        for (kind, manifest) in manifests(source) {
            count += self.import_manifest_entries(&id, &target_dir, source, &kind, &manifest)?;
        }
        Ok(TemplatePackSummary {
            id,
            name,
            version: None,
            template_count: count,
        })
    }

    pub fn template_detail(&self, id: &str) -> AppResult<TemplateDetail> {
        let conn = self.store.connection()?;
        let mut stmt = conn.prepare(
            "SELECT id, source_id, kind, category, rounded_ratio, visual_intent, content_summary, image_path \
             FROM templates WHERE id = ?1",
        )?;
        stmt.query_row(params![id], |row| {
            let image_path = PathBuf::from(row.get::<_, String>(7)?);
            Ok(TemplateDetail {
                id: row.get(0)?,
                source_id: row.get(1)?,
                kind: row.get(2)?,
                category: row.get(3)?,
                rounded_ratio: row.get(4)?,
                visual_intent: row.get(5)?,
                content_summary: row.get(6)?,
                mime_type: guess_mime(&image_path),
                image_path,
            })
        })
        .map_err(|error| match error {
            rusqlite::Error::QueryReturnedNoRows => {
                AppError::new("template_not_found", "Template not found")
            }
            other => other.into(),
        })
    }

    pub fn template_file(&self, id: &str) -> AppResult<TemplateFile> {
        let conn = self.store.connection()?;
        let mut stmt = conn.prepare("SELECT id, image_path FROM templates WHERE id = ?1")?;
        stmt.query_row(params![id], |row| {
            let path = PathBuf::from(row.get::<_, String>(1)?);
            Ok(TemplateFile {
                id: row.get(0)?,
                mime_type: guess_mime(&path),
                path,
            })
        })
        .map_err(|error| match error {
            rusqlite::Error::QueryReturnedNoRows => {
                AppError::new("template_not_found", "Template not found")
            }
            other => other.into(),
        })
    }

    pub fn delete_templates(&self, ids: &[String]) -> AppResult<usize> {
        let mut removed = 0_usize;
        let mut directories: Vec<PathBuf> = Vec::new();
        let conn = self.store.connection()?;
        for id in ids {
            let image_path: Option<String> = conn
                .query_row(
                    "SELECT image_path FROM templates WHERE id = ?1",
                    params![id],
                    |row| row.get(0),
                )
                .ok();
            let Some(image_path) = image_path else {
                continue;
            };
            let affected = conn.execute("DELETE FROM templates WHERE id = ?1", params![id])?;
            if affected == 0 {
                continue;
            }
            removed += affected;
            let image_path = PathBuf::from(image_path);
            let _ = std::fs::remove_file(&image_path);
            if let Some(parent) = image_path.parent() {
                if parent.starts_with(self.app_data.join("templates"))
                    && !directories.iter().any(|dir| dir == parent)
                {
                    directories.push(parent.to_path_buf());
                }
            }
        }
        for dir in directories {
            let empty = std::fs::read_dir(&dir)
                .map(|mut entries| entries.next().is_none())
                .unwrap_or(false);
            if empty {
                let _ = std::fs::remove_dir(&dir);
            }
        }
        conn.execute(
            "DELETE FROM template_packs WHERE id NOT IN \
             (SELECT DISTINCT pack_id FROM templates WHERE pack_id IS NOT NULL)",
            [],
        )?;
        Ok(removed)
    }

    fn import_manifest_entries(
        &self,
        pack_id: &str,
        target_dir: &Path,
        source_dir: &Path,
        fallback_kind: &str,
        manifest_path: &Path,
    ) -> AppResult<i64> {
        let data = std::fs::read_to_string(manifest_path)?;
        let entries = serde_json::from_str::<Value>(&data)?;
        let Some(items) = entries.as_array() else {
            return Ok(0);
        };
        let mut count = 0_i64;
        let conn = self.store.connection()?;
        for item in items {
            let raw_id = string_field(item, "id").unwrap_or_else(|| Uuid::new_v4().to_string());
            let kind = normalize_kind(
                &string_field(item, "kind").unwrap_or_else(|| fallback_kind.to_string()),
            );
            let Some(relative_image) = string_field(item, "path_to_gt_image")
                .or_else(|| string_field(item, "image_path"))
                .or_else(|| string_field(item, "image"))
            else {
                continue;
            };
            let image_source = source_dir.join(&kind).join(&relative_image);
            let image_source = if image_source.exists() {
                image_source
            } else {
                source_dir.join(&relative_image)
            };
            if !image_source.exists() {
                continue;
            }
            let template_id = Uuid::new_v4().to_string();
            let suffix = image_source
                .extension()
                .and_then(|value| value.to_str())
                .map(|value| format!(".{value}"))
                .unwrap_or_else(|| ".png".to_string());
            let image_target = target_dir.join(format!("{template_id}{suffix}"));
            std::fs::copy(&image_source, &image_target)?;
            let content_summary = item
                .get("content")
                .map(|value| value.to_string())
                .or_else(|| string_field(item, "content_summary"))
                .unwrap_or_default();
            conn.execute(
                "INSERT INTO templates(id, pack_id, source_id, kind, category, rounded_ratio, visual_intent, content_summary, image_path, created_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    template_id,
                    pack_id,
                    raw_id,
                    kind,
                    string_field(item, "category").or_else(|| string_field(item, "original_category")),
                    item.get("additional_info")
                        .and_then(|info| info.get("rounded_ratio"))
                        .and_then(Value::as_str),
                    string_field(item, "visual_intent").unwrap_or_default(),
                    content_summary,
                    image_target.to_string_lossy().to_string(),
                    Utc::now().to_rfc3339(),
                ],
            )?;
            count += 1;
        }
        Ok(count)
    }
}

fn manifests(source: &Path) -> Vec<(String, PathBuf)> {
    [
        (
            "diagram".to_string(),
            source.join("diagram").join("ref.json"),
        ),
        ("plot".to_string(), source.join("plot").join("ref.json")),
        ("diagram".to_string(), source.join("ref.json")),
    ]
    .into_iter()
    .filter(|(_, path)| path.exists())
    .collect()
}

fn normalize_kind(value: &str) -> String {
    match value.trim().to_ascii_lowercase().as_str() {
        "plot" => "plot".to_string(),
        "master" => "master".to_string(),
        _ => "diagram".to_string(),
    }
}

fn string_field(item: &Value, key: &str) -> Option<String> {
    item.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "dreampaper-tpl-test-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("建临时目录");
        dir
    }

    fn import(service: &TemplateService<'_>, name: &str, kind: &str, category: &str) -> String {
        service
            .import_template_image(
                format!("{name}.png"),
                "image/png".to_string(),
                vec![0x89, b'P', b'N', b'G'],
                kind.to_string(),
                Some(category.to_string()),
                Some(format!("intent for {name}")),
                Some(format!("summary for {name}")),
            )
            .expect("导入应成功")
            .id
    }

    #[test]
    fn listing_is_not_capped_and_keeps_the_oldest_entries() {
        let dir = scratch("cap");
        let store = Store::initialize(&dir).expect("初始化 store");
        let service = TemplateService::new(&store, &dir);

        let first = import(&service, "oldest", "diagram", "流程图");
        for index in 1..70 {
            import(&service, &format!("t{index}"), "diagram", "流程图");
        }

        let listed = service
            .list_templates("all".to_string(), String::new())
            .expect("列表应成功");
        assert_eq!(listed.len(), 70, "70 张应全部列出，不该被 60 截断");
        assert!(
            listed.iter().any(|item| item.id == first),
            "最早导入的那张不该被挤出列表"
        );
    }

    #[test]
    fn filters_by_kind_and_query() {
        let dir = scratch("filter");
        let store = Store::initialize(&dir).expect("初始化 store");
        let service = TemplateService::new(&store, &dir);

        import(&service, "alpha", "diagram", "流程图");
        import(&service, "beta", "plot", "折线图");
        let master = import(&service, "gamma", "master", "学术蓝");

        let masters = service
            .list_templates("master".to_string(), String::new())
            .expect("列表应成功");
        assert_eq!(masters.len(), 1);
        assert_eq!(masters[0].id, master);

        assert_eq!(
            service
                .list_templates(String::new(), String::new())
                .expect("列表应成功")
                .len(),
            3
        );

        let hits = service
            .list_templates("all".to_string(), "折线".to_string())
            .expect("列表应成功");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].category.as_deref(), Some("折线图"));

        let hits = service
            .list_templates("all".to_string(), "ALPHA".to_string())
            .expect("列表应成功");
        assert_eq!(hits.len(), 1);
        assert!(hits[0].visual_intent.contains("alpha"));

        assert!(service
            .list_templates("master".to_string(), "折线".to_string())
            .expect("列表应成功")
            .is_empty());
    }

    #[test]
    fn delete_removes_rows_and_files_and_tolerates_unknown_ids() {
        let dir = scratch("delete");
        let store = Store::initialize(&dir).expect("初始化 store");
        let service = TemplateService::new(&store, &dir);

        let doomed = import(&service, "doomed", "diagram", "流程图");
        let kept = import(&service, "kept", "master", "学术蓝");
        let doomed_path = service
            .template_detail(&doomed)
            .expect("详情应成功")
            .image_path;
        assert!(doomed_path.exists());

        let removed = service
            .delete_templates(&[doomed.clone(), "not-a-real-id".to_string()])
            .expect("删除应成功");
        assert_eq!(removed, 1, "不存在的 id 不该计入删除数");
        assert!(!doomed_path.exists(), "磁盘上的图片也要清掉");
        assert!(
            !doomed_path.parent().expect("有父目录").exists(),
            "单张导入独占的目录空了就该删掉"
        );

        let listed = service
            .list_templates("all".to_string(), String::new())
            .expect("列表应成功");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, kept);

        assert_eq!(service.delete_templates(&[doomed]).expect("删除应成功"), 0);
    }
}
