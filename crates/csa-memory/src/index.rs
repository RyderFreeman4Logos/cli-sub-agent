use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tantivy::{
    DateTime as TantivyDateTime, Index, IndexReader, IndexWriter, ReloadPolicy, TantivyDocument,
    collector::TopDocs,
    directory::MmapDirectory,
    doc,
    query::QueryParser,
    schema::{DateOptions, Field, STORED, STRING, Schema, TEXT, Value},
};

use crate::MemoryEntry;

const WRITER_HEAP_BYTES: usize = 50_000_000;
const SNIPPET_MAX_CHARS: usize = 180;

#[derive(Debug, Clone)]
pub struct SearchResult {
    pub entry_id: String,
    pub score: f32,
    pub snippet: String,
}

pub struct MemoryIndex {
    index: Index,
    reader: IndexReader,
    writer_path: PathBuf,
    field_ulid: Field,
    field_timestamp: Field,
    field_project: Field,
    field_tool: Field,
    field_content: Field,
    field_facts: Field,
    field_tags: Field,
}

struct IndexFields {
    ulid: Field,
    timestamp: Field,
    project: Field,
    tool: Field,
    content: Field,
    facts: Field,
    tags: Field,
}

impl MemoryIndex {
    pub fn open(index_dir: &Path) -> Result<Self> {
        fs::create_dir_all(index_dir).with_context(|| {
            format!("failed to create index directory: {}", index_dir.display())
        })?;

        let (schema, fields) = build_schema();
        let directory = MmapDirectory::open(index_dir)
            .with_context(|| format!("failed to open mmap directory: {}", index_dir.display()))?;
        let index = Index::open_or_create(directory, schema).with_context(|| {
            format!("failed to open or create index at {}", index_dir.display())
        })?;
        let reader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::Manual)
            .try_into()
            .context("failed to build tantivy reader with manual reload policy")?;

        Ok(Self {
            index,
            reader,
            writer_path: index_dir.to_path_buf(),
            field_ulid: fields.ulid,
            field_timestamp: fields.timestamp,
            field_project: fields.project,
            field_tool: fields.tool,
            field_content: fields.content,
            field_facts: fields.facts,
            field_tags: fields.tags,
        })
    }

    pub fn index_entry(&self, entry: &MemoryEntry) -> Result<()> {
        let mut writer = self.open_writer()?;
        writer
            .add_document(self.entry_document(entry))
            .context("failed to add memory entry document")?;
        writer.commit().context("failed to commit indexed entry")?;
        self.reader
            .reload()
            .context("failed to reload reader after indexing entry")?;
        Ok(())
    }

    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>> {
        if query.trim().is_empty() || limit == 0 {
            return Ok(Vec::new());
        }

        let query_parser = QueryParser::for_index(
            &self.index,
            vec![self.field_content, self.field_facts, self.field_tags],
        );
        let parsed_query = query_parser
            .parse_query(query)
            .with_context(|| format!("failed to parse query: {query}"))?;

        let searcher = self.reader.searcher();
        let top_docs = searcher
            .search(&parsed_query, &TopDocs::with_limit(limit))
            .context("failed to execute BM25 search")?;

        let mut results = Vec::with_capacity(top_docs.len());
        for (score, address) in top_docs {
            let doc: TantivyDocument = searcher
                .doc(address)
                .with_context(|| format!("failed to load document at address {address:?}"))?;
            let entry_id = doc
                .get_first(self.field_ulid)
                .and_then(|value| value.as_str())
                .unwrap_or_default()
                .to_string();

            results.push(SearchResult {
                entry_id,
                score,
                snippet: self.build_snippet(query, &doc),
            });
        }

        Ok(results)
    }

    pub fn rebuild(&self, entries: &[MemoryEntry]) -> Result<()> {
        let mut writer = self.open_writer()?;
        writer
            .delete_all_documents()
            .context("failed to clear index before rebuild")?;

        for entry in entries {
            writer
                .add_document(self.entry_document(entry))
                .context("failed to add entry during index rebuild")?;
        }

        writer
            .commit()
            .context("failed to commit rebuilt memory index")?;
        self.reader
            .reload()
            .context("failed to reload reader after rebuild")?;
        Ok(())
    }

    fn open_writer(&self) -> Result<IndexWriter> {
        self.index.writer(WRITER_HEAP_BYTES).with_context(|| {
            format!(
                "failed to create index writer at {}",
                self.writer_path.display()
            )
        })
    }

    fn entry_document(&self, entry: &MemoryEntry) -> TantivyDocument {
        let project = entry.project.as_deref().unwrap_or_default();
        let tool = entry.tool.as_deref().unwrap_or_default();
        let facts = entry.facts.join("\n");
        let tags = entry.tags.join(" ");
        let timestamp = TantivyDateTime::from_timestamp_secs(entry.timestamp.timestamp());

        doc!(
            self.field_ulid => entry.id.to_string(),
            self.field_timestamp => timestamp,
            self.field_project => project,
            self.field_tool => tool,
            self.field_content => entry.content.as_str(),
            self.field_facts => facts,
            self.field_tags => tags,
        )
    }

    fn build_snippet(&self, query: &str, doc: &TantivyDocument) -> String {
        let terms: Vec<String> = query
            .split_whitespace()
            .map(|item| item.to_ascii_lowercase())
            .filter(|item| !item.is_empty())
            .collect();

        for field in [self.field_content, self.field_facts, self.field_tags] {
            if let Some(text) = doc
                .get_first(field)
                .and_then(|value| value.as_str())
                .filter(|text| !text.is_empty())
            {
                let lower = text.to_ascii_lowercase();
                if terms.is_empty() || terms.iter().any(|term| lower.contains(term)) {
                    return truncate_snippet(text);
                }
            }
        }

        String::new()
    }
}

fn build_schema() -> (Schema, IndexFields) {
    let mut schema_builder = Schema::builder();
    let field_ulid = schema_builder.add_text_field("ulid", STRING | STORED);

    let timestamp_options = DateOptions::default().set_stored().set_indexed();
    let field_timestamp = schema_builder.add_date_field("timestamp", timestamp_options);

    let field_project = schema_builder.add_text_field("project", TEXT | STORED);
    let field_tool = schema_builder.add_text_field("tool", STRING | STORED);
    let field_content = schema_builder.add_text_field("content", TEXT | STORED);
    let field_facts = schema_builder.add_text_field("facts", TEXT | STORED);
    let field_tags = schema_builder.add_text_field("tags", TEXT | STORED);

    (
        schema_builder.build(),
        IndexFields {
            ulid: field_ulid,
            timestamp: field_timestamp,
            project: field_project,
            tool: field_tool,
            content: field_content,
            facts: field_facts,
            tags: field_tags,
        },
    )
}

fn truncate_snippet(text: &str) -> String {
    if text.chars().count() <= SNIPPET_MAX_CHARS {
        return text.to_string();
    }

    let mut truncated: String = text.chars().take(SNIPPET_MAX_CHARS).collect();
    truncated.push_str("...");
    truncated
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use ulid::Ulid;

    use crate::MemorySource;

    fn make_index_dir() -> PathBuf {
        std::env::temp_dir().join(format!("csa-memory-index-test-{}", Ulid::new()))
    }

    fn make_entry(content: &str, facts: &[&str], tags: &[&str]) -> MemoryEntry {
        MemoryEntry {
            id: Ulid::new(),
            timestamp: Utc::now(),
            project: Some("test-project".to_string()),
            tool: Some("codex".to_string()),
            session_id: Some(format!("session-{}", Ulid::new())),
            tags: tags.iter().map(|tag| (*tag).to_string()).collect(),
            content: content.to_string(),
            facts: facts.iter().map(|fact| (*fact).to_string()).collect(),
            source: MemorySource::PostRun,
            valid_from: None,
            valid_until: None,
        }
    }

    #[test]
    fn test_index_and_search() -> Result<()> {
        let index_dir = make_index_dir();
        let index = MemoryIndex::open(&index_dir)?;

        let e1 = make_entry("rust rust borrow checker details", &[], &["language"]);
        let e2 = make_entry("python scripting guide", &[], &["python"]);
        let e3 = make_entry("rust async tokio runtime", &[], &["async"]);

        index.index_entry(&e1)?;
        index.index_entry(&e2)?;
        index.index_entry(&e3)?;

        let results = index.search("rust", 10)?;
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].entry_id, e1.id.to_string());
        assert!(results[0].score >= results[1].score);

        fs::remove_dir_all(index_dir).ok();
        Ok(())
    }

    #[test]
    fn test_search_no_results() -> Result<()> {
        let index_dir = make_index_dir();
        let index = MemoryIndex::open(&index_dir)?;
        index.index_entry(&make_entry("hello world", &[], &[]))?;

        let results = index.search("not-present-keyword", 10)?;
        assert!(results.is_empty());

        fs::remove_dir_all(index_dir).ok();
        Ok(())
    }

    #[test]
    fn test_rebuild_index() -> Result<()> {
        let index_dir = make_index_dir();
        let index = MemoryIndex::open(&index_dir)?;

        let old_entry = make_entry("legacy memory token", &[], &[]);
        index.index_entry(&old_entry)?;
        assert_eq!(index.search("legacy", 10)?.len(), 1);

        let new_entry = make_entry("modern memory record", &[], &[]);
        index.rebuild(std::slice::from_ref(&new_entry))?;

        assert!(index.search("legacy", 10)?.is_empty());
        let modern_results = index.search("modern", 10)?;
        assert_eq!(modern_results.len(), 1);
        assert_eq!(modern_results[0].entry_id, new_entry.id.to_string());

        fs::remove_dir_all(index_dir).ok();
        Ok(())
    }

    #[test]
    fn test_search_across_fields() -> Result<()> {
        let index_dir = make_index_dir();
        let index = MemoryIndex::open(&index_dir)?;

        let fact_entry = make_entry(
            "no match in body",
            &["critical keyword lives in facts"],
            &[],
        );
        index.index_entry(&fact_entry)?;

        let results = index.search("keyword", 10)?;
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].entry_id, fact_entry.id.to_string());

        fs::remove_dir_all(index_dir).ok();
        Ok(())
    }

    #[test]
    fn test_search_ranking() -> Result<()> {
        let index_dir = make_index_dir();
        let index = MemoryIndex::open(&index_dir)?;

        let low = make_entry("alpha beta", &[], &[]);
        let high = make_entry("alpha alpha alpha beta", &[], &[]);
        index.index_entry(&low)?;
        index.index_entry(&high)?;

        let results = index.search("alpha", 10)?;
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].entry_id, high.id.to_string());
        assert!(results[0].score > results[1].score);

        fs::remove_dir_all(index_dir).ok();
        Ok(())
    }
}
