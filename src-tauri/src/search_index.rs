use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use std::{fmt, path::Path, time::Duration};

const SCHEMA_VERSION: i64 = 1;
const MAX_SEARCH_RESULTS: usize = 100;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IndexDocument {
    pub relative_path: String,
    pub display_title: String,
    pub content_hash: String,
    pub searchable_content: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SearchResult {
    pub relative_path: String,
    pub display_title: String,
    pub snippet: String,
    pub score: f64,
}

#[derive(Debug)]
pub enum SearchIndexError {
    Database(rusqlite::Error),
    InvalidDocument,
    UnsupportedSchema { found: i64, supported: i64 },
}

impl fmt::Display for SearchIndexError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Database(_) => formatter.write_str("The search index is unavailable."),
            Self::InvalidDocument => formatter.write_str("The index document is invalid."),
            Self::UnsupportedSchema { found, supported } => write!(
                formatter,
                "Search index schema {found} is incompatible with supported schema {supported}."
            ),
        }
    }
}

impl std::error::Error for SearchIndexError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Database(error) => Some(error),
            Self::InvalidDocument | Self::UnsupportedSchema { .. } => None,
        }
    }
}

impl From<rusqlite::Error> for SearchIndexError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Database(error)
    }
}

pub type SearchIndexResult<T> = Result<T, SearchIndexError>;

pub struct SearchIndex {
    connection: Connection,
}

impl SearchIndex {
    pub fn open(path: &Path) -> SearchIndexResult<Self> {
        let connection = Connection::open(path)?;
        Self::initialize(connection)
    }

    pub fn open_in_memory() -> SearchIndexResult<Self> {
        let connection = Connection::open_in_memory()?;
        Self::initialize(connection)
    }

    fn initialize(connection: Connection) -> SearchIndexResult<Self> {
        connection.busy_timeout(Duration::from_secs(2))?;
        connection.execute_batch(
            "PRAGMA foreign_keys = ON;
             PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA temp_store = MEMORY;
             CREATE TABLE IF NOT EXISTS schema_meta (
                 key TEXT PRIMARY KEY,
                 value INTEGER NOT NULL
             ) STRICT;",
        )?;

        let existing_version = connection
            .query_row(
                "SELECT value FROM schema_meta WHERE key = 'schema_version'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .optional()?;
        if let Some(found) = existing_version {
            if found != SCHEMA_VERSION {
                return Err(SearchIndexError::UnsupportedSchema {
                    found,
                    supported: SCHEMA_VERSION,
                });
            }
        }

        connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS notes (
                 relative_path TEXT PRIMARY KEY,
                 display_title TEXT NOT NULL,
                 content_hash TEXT NOT NULL
             ) STRICT;
             CREATE VIRTUAL TABLE IF NOT EXISTS fts_notes USING fts5(
                 relative_path,
                 title,
                 searchable_content,
                 tokenize = 'unicode61 remove_diacritics 2',
                 prefix = '2 3 4'
             );
             INSERT INTO schema_meta(key, value)
             VALUES ('schema_version', 1)
             ON CONFLICT(key) DO NOTHING;",
        )?;

        Ok(Self { connection })
    }

    pub fn rebuild(&mut self, documents: &[IndexDocument]) -> SearchIndexResult<()> {
        validate_documents(documents)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute("DELETE FROM fts_notes", [])?;
        transaction.execute("DELETE FROM notes", [])?;
        {
            let mut note_statement = transaction.prepare_cached(
                "INSERT INTO notes(relative_path, display_title, content_hash)
                 VALUES (?1, ?2, ?3)",
            )?;
            let mut fts_statement = transaction.prepare_cached(
                "INSERT INTO fts_notes(relative_path, title, searchable_content)
                 VALUES (?1, ?2, ?3)",
            )?;
            for document in documents {
                note_statement.execute(params![
                    document.relative_path,
                    document.display_title,
                    document.content_hash
                ])?;
                fts_statement.execute(params![
                    document.relative_path,
                    document.display_title,
                    document.searchable_content
                ])?;
            }
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn upsert_note(&mut self, document: &IndexDocument) -> SearchIndexResult<()> {
        validate_document(document)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        upsert_document(&transaction, document)?;
        transaction.commit()?;
        Ok(())
    }

    pub fn delete_note(&mut self, relative_path: &str) -> SearchIndexResult<()> {
        if relative_path.is_empty() {
            return Err(SearchIndexError::InvalidDocument);
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute(
            "DELETE FROM fts_notes WHERE relative_path = ?1",
            [relative_path],
        )?;
        transaction.execute(
            "DELETE FROM notes WHERE relative_path = ?1",
            [relative_path],
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn search(&self, raw_query: &str, limit: usize) -> SearchIndexResult<Vec<SearchResult>> {
        let Some(match_query) = build_match_query(raw_query) else {
            return Ok(Vec::new());
        };
        let limit = limit.clamp(1, MAX_SEARCH_RESULTS) as i64;
        let mut statement = self.connection.prepare_cached(
            "SELECT relative_path,
                    title,
                    snippet(fts_notes, 2, '[', ']', ' … ', 18),
                    bm25(fts_notes, 4.0, 8.0, 1.0) AS score
             FROM fts_notes
             WHERE fts_notes MATCH ?1
             ORDER BY CASE
                        WHEN title = ?2 THEN 0
                        WHEN relative_path = ?2 THEN 1
                        ELSE 2
                      END,
                      score,
                      relative_path
             LIMIT ?3",
        )?;
        let rows = statement.query_map(params![match_query, raw_query.trim(), limit], |row| {
            Ok(SearchResult {
                relative_path: row.get(0)?,
                display_title: row.get(1)?,
                snippet: row.get(2)?,
                score: row.get(3)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn note_count(&self) -> SearchIndexResult<usize> {
        let count = self
            .connection
            .query_row("SELECT COUNT(*) FROM notes", [], |row| row.get::<_, i64>(0))?;
        usize::try_from(count).map_err(|_| {
            SearchIndexError::Database(rusqlite::Error::IntegralValueOutOfRange(0, count))
        })
    }

    pub fn optimize(&self) -> SearchIndexResult<()> {
        self.connection
            .execute("INSERT INTO fts_notes(fts_notes) VALUES ('optimize')", [])?;
        self.connection
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
        Ok(())
    }
}

fn upsert_document(
    transaction: &Transaction<'_>,
    document: &IndexDocument,
) -> SearchIndexResult<()> {
    transaction.execute(
        "INSERT INTO notes(relative_path, display_title, content_hash)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(relative_path) DO UPDATE SET
             display_title = excluded.display_title,
             content_hash = excluded.content_hash",
        params![
            document.relative_path,
            document.display_title,
            document.content_hash
        ],
    )?;
    transaction.execute(
        "DELETE FROM fts_notes WHERE relative_path = ?1",
        [&document.relative_path],
    )?;
    transaction.execute(
        "INSERT INTO fts_notes(relative_path, title, searchable_content)
         VALUES (?1, ?2, ?3)",
        params![
            document.relative_path,
            document.display_title,
            document.searchable_content
        ],
    )?;
    Ok(())
}

fn validate_documents(documents: &[IndexDocument]) -> SearchIndexResult<()> {
    for document in documents {
        validate_document(document)?;
    }
    Ok(())
}

fn validate_document(document: &IndexDocument) -> SearchIndexResult<()> {
    let valid_hash = document.content_hash.len() == 64
        && document
            .content_hash
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit());
    if document.relative_path.is_empty() || document.display_title.is_empty() || !valid_hash {
        return Err(SearchIndexError::InvalidDocument);
    }
    Ok(())
}

fn build_match_query(raw_query: &str) -> Option<String> {
    let terms: Vec<_> = raw_query
        .split_whitespace()
        .filter_map(|term| {
            let cleaned = term.trim_matches(|character: char| !character.is_alphanumeric());
            (!cleaned.is_empty()).then(|| format!("\"{}\"*", cleaned.replace('"', "\"\"")))
        })
        .collect();
    (!terms.is_empty()).then(|| terms.join(" AND "))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn document(path: &str, title: &str, content: &str, hash_digit: char) -> IndexDocument {
        IndexDocument {
            relative_path: path.to_owned(),
            display_title: title.to_owned(),
            content_hash: std::iter::repeat_n(hash_digit, 64).collect(),
            searchable_content: content.to_owned(),
        }
    }

    #[test]
    fn vietnamese_search_is_case_and_diacritic_insensitive() {
        let mut index = SearchIndex::open_in_memory().expect("index should open");
        index
            .rebuild(&[
                document(
                    "Dự án/Kế hoạch.md",
                    "Kế hoạch nghiên cứu",
                    "Phân tích dữ liệu tiếng Việt và lộ trình phát triển.",
                    'a',
                ),
                document("notes/other.md", "Other", "Unrelated text", 'b'),
            ])
            .expect("fixture should index");

        for query in ["KẾ HOẠCH", "ke hoach", "nghien cuu", "phân tích"] {
            let results = index.search(query, 10).expect("query should succeed");
            assert_eq!(results[0].relative_path, "Dự án/Kế hoạch.md");
        }
    }

    #[test]
    fn filename_title_content_and_prefix_are_searchable() {
        let mut index = SearchIndex::open_in_memory().expect("index should open");
        index
            .rebuild(&[
                document("Nhật ký.md", "Nhật ký", "Ghi lại buổi họp", 'a'),
                document("Khác.md", "Ghi chú", "nhật thực", 'b'),
            ])
            .expect("fixture should index");

        assert_eq!(
            index.search("Nhật ký", 10).expect("title query")[0].relative_path,
            "Nhật ký.md"
        );
        assert_eq!(
            index.search("buổi họ", 10).expect("prefix query")[0].relative_path,
            "Nhật ký.md"
        );
    }

    #[test]
    fn incremental_update_and_delete_touch_only_the_target_note() {
        let mut index = SearchIndex::open_in_memory().expect("index should open");
        let first = document("first.md", "First", "nội dung cũ", 'a');
        let second = document("second.md", "Second", "giữ nguyên", 'b');
        index
            .rebuild(&[first.clone(), second])
            .expect("fixture should index");

        let updated = document("first.md", "First", "nội dung mới độc nhất", 'c');
        index.upsert_note(&updated).expect("note should update");
        assert!(index.search("cũ", 10).expect("old query").is_empty());
        assert_eq!(index.search("độc nhất", 10).expect("new query").len(), 1);
        assert_eq!(
            index.search("giữ nguyên", 10).expect("stable query").len(),
            1
        );

        index.delete_note("first.md").expect("note should delete");
        assert!(index
            .search("độc nhất", 10)
            .expect("deleted query")
            .is_empty());
        assert_eq!(index.note_count().expect("count should work"), 1);
    }

    #[test]
    fn invalid_update_preserves_the_previous_index_row() {
        let mut index = SearchIndex::open_in_memory().expect("index should open");
        let original = document("note.md", "Note", "nội dung an toàn", 'a');
        index
            .rebuild(std::slice::from_ref(&original))
            .expect("fixture should index");
        let invalid = IndexDocument {
            content_hash: "bad".to_owned(),
            searchable_content: "should not replace".to_owned(),
            ..original
        };

        assert!(matches!(
            index.upsert_note(&invalid),
            Err(SearchIndexError::InvalidDocument)
        ));
        assert_eq!(index.search("an toàn", 10).expect("old row query").len(), 1);
        assert!(index
            .search("replace", 10)
            .expect("new row query")
            .is_empty());
    }

    #[test]
    fn full_rebuild_removes_stale_rows_and_can_recreate_a_deleted_database() {
        let temp = tempfile::tempdir().expect("fixture should open");
        let database = temp.path().join("search.sqlite3");
        let original = document("old.md", "Old", "stale token", 'a');
        {
            let mut index = SearchIndex::open(&database).expect("index should open");
            index
                .rebuild(std::slice::from_ref(&original))
                .expect("fixture should index");
            index.rebuild(&[]).expect("empty rebuild should work");
            assert_eq!(index.note_count().expect("count should work"), 0);
        }
        std::fs::remove_file(&database).expect("cache database should be removable");

        let mut recreated = SearchIndex::open(&database).expect("index should recreate");
        recreated
            .rebuild(std::slice::from_ref(&original))
            .expect("recreated index should rebuild");
        assert_eq!(recreated.search("stale", 10).expect("query").len(), 1);
    }

    #[test]
    fn incompatible_schema_is_rejected_for_caller_directed_rebuild() {
        let temp = tempfile::tempdir().expect("fixture should open");
        let database = temp.path().join("search.sqlite3");
        {
            let index = SearchIndex::open(&database).expect("index should open");
            index
                .connection
                .execute(
                    "UPDATE schema_meta SET value = 99 WHERE key = 'schema_version'",
                    [],
                )
                .expect("schema fixture should change");
        }

        assert!(matches!(
            SearchIndex::open(&database),
            Err(SearchIndexError::UnsupportedSchema {
                found: 99,
                supported: SCHEMA_VERSION
            })
        ));
    }

    #[test]
    fn arbitrary_query_syntax_is_quoted_and_limited() {
        let mut index = SearchIndex::open_in_memory().expect("index should open");
        index
            .rebuild(&[document("note.md", "Note", "alpha beta", 'a')])
            .expect("fixture should index");

        assert!(index.search("   ", 10).expect("empty query").is_empty());
        assert_eq!(
            index.search("alpha OR \"", 10).expect("quoted query").len(),
            0
        );
    }
}
