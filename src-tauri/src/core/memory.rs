//! Case memory: design products of finished jobs, recalled for new ones.
//!
//! A case is the design-stage output of a job that reached the implement
//! stage, keyed by a task fingerprint (mode plus the user brief). New jobs
//! filter by mode and rank the rest by full-text similarity of the brief; the
//! hits go to the advisor step in `advisor.rs`.
//!
//! FTS5's default `unicode61` tokenizer treats a run of CJK characters as one
//! token, so a Chinese brief would only ever match an identical brief. Both
//! the indexed text and the query are therefore rewritten as CJK bigrams
//! ("图检索" -> "图检 检索") before they touch FTS5; ASCII words are kept
//! whole. Rewriting both sides with the same function is what keeps recall
//! consistent.

use chrono::Utc;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::core::store::Store;
use crate::error::{AppError, AppResult};

/// How many hits the advisor is shown at most.
pub const RECALL_LIMIT: usize = 3;

/// Cap on how much of a case's design text goes into the advisor prompt.
const DESIGN_EXCERPT_CHARS: usize = 3500;

/// User feedback on a finished task.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Rating {
    Good,
    Fair,
    Poor,
}

impl Rating {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "good" => Some(Self::Good),
            "fair" => Some(Self::Fair),
            "poor" => Some(Self::Poor),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Good => "good",
            Self::Fair => "fair",
            Self::Poor => "poor",
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct MemoryCase {
    pub job_id: String,
    pub mode: String,
    pub title: String,
    pub brief: String,
    pub design: String,
    pub rating: Option<Rating>,
    pub created_at: String,
    /// bm25 score from FTS5: lower is a closer match.
    pub score: f64,
}

/// The mode filter and query text a job contributes to recall.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fingerprint {
    pub mode: String,
    pub title: String,
    pub brief: String,
}

impl Fingerprint {
    /// Built from the job payload envelope (`{"mode", "payload": {...}}`).
    pub fn from_payload(envelope: &Value) -> Option<Self> {
        let mode = envelope.get("mode")?.as_str()?.to_string();
        let payload = envelope.get("payload").unwrap_or(envelope);
        let field = |key: &str| {
            payload
                .get(key)
                .and_then(Value::as_str)
                .map(str::trim)
                .unwrap_or_default()
                .to_string()
        };
        let (title, brief) = match mode.as_str() {
            "ppt_slide" => {
                let material = field("material_text");
                let title = material
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .trim()
                    .to_string();
                (title, material)
            }
            _ => (field("figure_title"), field("section_description")),
        };
        if title.is_empty() && brief.is_empty() {
            return None;
        }
        Some(Self { mode, title, brief })
    }

    fn query_text(&self) -> String {
        format!("{} {}", self.title, self.brief)
    }
}

/// Rewrites text so FTS5's `unicode61` tokenizer sees CJK bigrams.
///
/// Consecutive CJK characters become overlapping pairs; a lone CJK character
/// stays as it is; everything else (ASCII words, digits) passes through and
/// is split by the tokenizer as usual.
pub fn cjk_bigrams(text: &str) -> String {
    let mut out = String::with_capacity(text.len() * 2);
    let mut run: Vec<char> = Vec::new();
    let flush = |run: &mut Vec<char>, out: &mut String| {
        match run.len() {
            0 => {}
            1 => {
                out.push(run[0]);
                out.push(' ');
            }
            _ => {
                for pair in run.windows(2) {
                    out.push(pair[0]);
                    out.push(pair[1]);
                    out.push(' ');
                }
            }
        }
        run.clear();
    };
    for ch in text.chars() {
        if is_cjk(ch) {
            run.push(ch);
        } else {
            flush(&mut run, &mut out);
            out.push(ch);
        }
    }
    flush(&mut run, &mut out);
    out
}

fn is_cjk(ch: char) -> bool {
    matches!(
        ch as u32,
        0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0x20000..=0x2FA1F
            | 0x3040..=0x30FF | 0xAC00..=0xD7AF
    )
}

/// Turns free text into an FTS5 `MATCH` expression: bigram-rewritten terms,
/// each quoted, joined with OR. Empty when nothing is searchable.
pub fn fts_query(text: &str) -> String {
    let rewritten = cjk_bigrams(text);
    let mut terms: Vec<String> = Vec::new();
    for part in rewritten.split(|ch: char| !(ch.is_alphanumeric() || ch == '_')) {
        if part.is_empty() {
            continue;
        }
        let term = format!("\"{}\"", part.replace('"', " "));
        if !terms.contains(&term) {
            terms.push(term);
        }
        if terms.len() >= 48 {
            break;
        }
    }
    terms.join(" OR ")
}

pub struct MemoryService<'a> {
    store: &'a Store,
}

impl<'a> MemoryService<'a> {
    pub fn new(store: &'a Store) -> Self {
        Self { store }
    }

    /// Sediment a finished job's design product. Re-recording a job replaces
    /// its case, so a rerun that reached implement again does not double up.
    pub fn record(&self, job_id: &str, fingerprint: &Fingerprint, design: &Value) -> AppResult<()> {
        let design_text = serde_json::to_string(design)?;
        let now = Utc::now().to_rfc3339();
        let index_text = cjk_bigrams(&format!("{} {}", fingerprint.title, fingerprint.brief));
        let mut conn = self.store.connection()?;
        let tx = conn.transaction()?;
        tx.execute(
            "DELETE FROM memory_fts WHERE rowid IN (SELECT rowid FROM memory_cases WHERE job_id = ?1)",
            params![job_id],
        )?;
        tx.execute(
            "DELETE FROM memory_cases WHERE job_id = ?1",
            params![job_id],
        )?;
        tx.execute(
            "INSERT INTO memory_cases(job_id, mode, title, brief, design_json, rating, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, NULL, ?6)",
            params![
                job_id,
                fingerprint.mode,
                fingerprint.title,
                fingerprint.brief,
                design_text,
                now
            ],
        )?;
        tx.execute(
            "INSERT INTO memory_fts(rowid, body) VALUES ((SELECT rowid FROM memory_cases WHERE job_id = ?1), ?2)",
            params![job_id, index_text],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Cases of the same mode ranked by brief similarity, best first.
    /// `exclude` keeps a rerun from advising itself with its own earlier case.
    pub fn recall(
        &self,
        fingerprint: &Fingerprint,
        exclude: Option<&str>,
        limit: usize,
    ) -> AppResult<Vec<MemoryCase>> {
        let query = fts_query(&fingerprint.query_text());
        if query.is_empty() {
            return Ok(Vec::new());
        }
        let conn = self.store.connection()?;
        let mut stmt = conn.prepare(
            "SELECT c.job_id, c.mode, c.title, c.brief, c.design_json, c.rating, c.created_at, bm25(memory_fts) AS score \
             FROM memory_fts JOIN memory_cases c ON c.rowid = memory_fts.rowid \
             WHERE memory_fts MATCH ?1 AND c.mode = ?2 AND c.job_id != ?3 \
             ORDER BY score LIMIT ?4",
        )?;
        let rows = stmt.query_map(
            params![
                query,
                fingerprint.mode,
                exclude.unwrap_or(""),
                limit.max(1) as i64
            ],
            |row| {
                Ok(MemoryCase {
                    job_id: row.get(0)?,
                    mode: row.get(1)?,
                    title: row.get(2)?,
                    brief: row.get(3)?,
                    design: row.get(4)?,
                    rating: row
                        .get::<_, Option<String>>(5)?
                        .as_deref()
                        .and_then(Rating::parse),
                    created_at: row.get(6)?,
                    score: row.get(7)?,
                })
            },
        )?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn rate(&self, job_id: &str, rating: Option<Rating>) -> AppResult<()> {
        let conn = self.store.connection()?;
        let changed = conn.execute(
            "UPDATE memory_cases SET rating = ?1 WHERE job_id = ?2",
            params![rating.map(Rating::as_str), job_id],
        )?;
        if changed == 0 {
            return Err(AppError::new(
                "memory_case_missing",
                "该任务没有沉淀为案例，无法评分",
            ));
        }
        Ok(())
    }

    pub fn rating(&self, job_id: &str) -> AppResult<Option<Rating>> {
        let conn = self.store.connection()?;
        let value: Option<Option<String>> = conn
            .query_row(
                "SELECT rating FROM memory_cases WHERE job_id = ?1",
                params![job_id],
                |row| row.get(0),
            )
            .optional()?;
        Ok(value.flatten().as_deref().and_then(Rating::parse))
    }

    pub fn has_case(&self, job_id: &str) -> AppResult<bool> {
        let conn = self.store.connection()?;
        let count: i64 = conn.query_row(
            "SELECT count(*) FROM memory_cases WHERE job_id = ?1",
            params![job_id],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    pub fn forget(&self, job_id: &str) -> AppResult<()> {
        let conn = self.store.connection()?;
        conn.execute(
            "DELETE FROM memory_fts WHERE rowid IN (SELECT rowid FROM memory_cases WHERE job_id = ?1)",
            params![job_id],
        )?;
        conn.execute(
            "DELETE FROM memory_cases WHERE job_id = ?1",
            params![job_id],
        )?;
        Ok(())
    }
}

/// The candidates as the advisor prompt sees them: brief, rating and a
/// bounded excerpt of the design product.
pub fn candidates_text(cases: &[MemoryCase]) -> String {
    cases
        .iter()
        .enumerate()
        .map(|(index, case)| {
            let rating = case
                .rating
                .map(|rating| rating.as_str().to_string())
                .unwrap_or_else(|| "unrated".to_string());
            let design: String = case.design.chars().take(DESIGN_EXCERPT_CHARS).collect();
            let truncated = if case.design.chars().count() > DESIGN_EXCERPT_CHARS {
                " …[truncated]"
            } else {
                ""
            };
            format!(
                "### Case {n}\nuser_rating: {rating}\ntitle: {title}\nbrief: {brief}\ndesign: {design}{truncated}",
                n = index + 1,
                title = case.title,
                brief = case.brief.chars().take(1200).collect::<String>(),
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn temp_store(tag: &str) -> (Store, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "dreampaper-memory-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        (Store::initialize(&dir).unwrap(), dir)
    }

    fn figure(title: &str, brief: &str) -> Fingerprint {
        Fingerprint {
            mode: "paper_figure".to_string(),
            title: title.to_string(),
            brief: brief.to_string(),
        }
    }

    #[test]
    fn bigrams_split_cjk_runs_and_keep_ascii_words() {
        assert_eq!(cjk_bigrams("图检索"), "图检 检索 ");
        assert_eq!(cjk_bigrams("图"), "图 ");
        assert_eq!(cjk_bigrams("BM25 双塔 rerank"), "BM25 双塔  rerank");
        let query = fts_query("多模态 检索 pipeline");
        assert_eq!(query, "\"多模\" OR \"模态\" OR \"检索\" OR \"pipeline\"");
        assert_eq!(fts_query("   "), "");
    }

    #[test]
    fn fingerprint_follows_the_mode() {
        let figure = Fingerprint::from_payload(&json!({
            "mode": "paper_figure",
            "payload": {"figure_title": " T ", "section_description": "body"}
        }))
        .unwrap();
        assert_eq!(figure.title, "T");
        assert_eq!(figure.brief, "body");

        let slide = Fingerprint::from_payload(&json!({
            "mode": "ppt_slide",
            "payload": {"material_text": "第一行\n更多资料"}
        }))
        .unwrap();
        assert_eq!(slide.title, "第一行");
        assert_eq!(slide.brief, "第一行\n更多资料");

        assert!(
            Fingerprint::from_payload(&json!({"mode": "paper_figure", "payload": {}})).is_none()
        );
    }

    #[test]
    fn chinese_briefs_are_recalled_by_partial_overlap_and_filtered_by_mode() {
        let (store, dir) = temp_store("recall");
        let memory = MemoryService::new(&store);
        memory
            .record(
                "j1",
                &figure(
                    "多模态文档检索流程",
                    "OCR 版面分析 双塔编码 BM25 交叉编码器 重排",
                ),
                &json!({"figure": {"title": "a"}}),
            )
            .unwrap();
        memory
            .record(
                "j2",
                &figure("蛋白质折叠预测", "序列比对 结构模板 能量最小化"),
                &json!({"figure": {"title": "b"}}),
            )
            .unwrap();
        memory
            .record(
                "s1",
                &Fingerprint {
                    mode: "ppt_slide".to_string(),
                    title: "文档检索汇报".to_string(),
                    brief: "文档检索 双塔 BM25 重排".to_string(),
                    ..figure("", "")
                },
                &json!({"deck": 1}),
            )
            .unwrap();

        // The new brief shares words but is not identical: the default
        // tokenizer would have found nothing here.
        let hits = memory
            .recall(
                &figure(
                    "跨模态检索系统",
                    "先做 OCR，再用 BM25 和双塔做检索，最后交叉编码器重排",
                ),
                None,
                RECALL_LIMIT,
            )
            .unwrap();
        assert_eq!(hits[0].job_id, "j1", "最相近的案例应排第一");
        assert!(
            hits.iter().all(|hit| hit.mode == "paper_figure"),
            "mode 必须硬过滤"
        );
        assert!(hits.iter().all(|hit| hit.job_id != "s1"));

        // A rerun must not be advised by its own earlier case.
        let hits = memory
            .recall(
                &figure("多模态文档检索流程", "OCR 版面分析 双塔编码"),
                Some("j1"),
                RECALL_LIMIT,
            )
            .unwrap();
        assert!(hits.iter().all(|hit| hit.job_id != "j1"));

        // Nothing in common: no candidates, no advisor call.
        let hits = memory
            .recall(&figure("量子退火", "哈密顿量 退火调度"), None, RECALL_LIMIT)
            .unwrap();
        assert!(hits.is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn ratings_round_trip_and_re_recording_replaces_the_case() {
        let (store, dir) = temp_store("rating");
        let memory = MemoryService::new(&store);
        assert!(memory.rate("missing", Some(Rating::Good)).is_err());
        memory
            .record("j1", &figure("标题", "内容"), &json!({"v": 1}))
            .unwrap();
        assert_eq!(memory.rating("j1").unwrap(), None);
        memory.rate("j1", Some(Rating::Poor)).unwrap();
        assert_eq!(memory.rating("j1").unwrap(), Some(Rating::Poor));
        memory.rate("j1", None).unwrap();
        assert_eq!(memory.rating("j1").unwrap(), None);

        memory.rate("j1", Some(Rating::Good)).unwrap();
        memory
            .record("j1", &figure("标题", "内容"), &json!({"v": 2}))
            .unwrap();
        let hits = memory.recall(&figure("标题", "内容"), None, 5).unwrap();
        assert_eq!(hits.len(), 1, "同一任务重录不该堆两条案例");
        assert!(hits[0].design.contains("\"v\":2"));
        assert_eq!(hits[0].rating, None, "重录后是新的设计产物，旧评分不再适用");
        assert!(memory.has_case("j1").unwrap());

        memory.forget("j1").unwrap();
        assert!(!memory.has_case("j1").unwrap());
        assert!(memory
            .recall(&figure("标题", "内容"), None, 5)
            .unwrap()
            .is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn candidate_text_bounds_design_length_and_names_the_rating() {
        let long = "x".repeat(DESIGN_EXCERPT_CHARS + 10);
        let text = candidates_text(&[MemoryCase {
            job_id: "j".into(),
            mode: "paper_figure".into(),
            title: "t".into(),
            brief: "b".into(),
            design: long,
            rating: Some(Rating::Fair),
            created_at: String::new(),
            score: -1.0,
        }]);
        assert!(text.contains("user_rating: fair"));
        assert!(text.contains("[truncated]"));
        assert!(text.chars().count() < DESIGN_EXCERPT_CHARS + 200);
    }
}
