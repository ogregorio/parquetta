use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub enum QueryInput {
    Filter {
        selected_columns: Vec<String>,
        where_clause: String,
    },
    Advanced {
        sql: String,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExportFormat {
    Csv,
    Parquet,
}

pub fn normalize_where_clause(input: &str) -> Result<String, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Ok(String::new());
    }

    if trimmed.contains(';') || trimmed.contains("--") || trimmed.contains("/*") {
        return Err("WHERE filter cannot contain ';' or SQL comments".to_string());
    }

    let without_where = trimmed
        .strip_prefix("WHERE ")
        .or_else(|| trimmed.strip_prefix("where "))
        .unwrap_or(trimmed);

    Ok(format!("WHERE {without_where}"))
}

pub fn quote_identifier(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}

pub fn preview_projection(columns: &[String]) -> String {
    columns
        .iter()
        .map(|column| {
            let quoted = quote_identifier(column);
            format!("CAST({quoted} AS VARCHAR) AS {quoted}")
        })
        .collect::<Vec<_>>()
        .join(", ")
}

pub fn advanced_sql(path: &Path, input: &str) -> Result<String, String> {
    let trimmed = input.trim().trim_end_matches(';').trim();
    if trimmed.is_empty() {
        return Err("Advanced query cannot be empty".to_string());
    }
    if !trimmed.contains("{{file}}") {
        return Err("Advanced query must include the {{file}} placeholder".to_string());
    }

    Ok(trimmed.replace("{{file}}", &format!("read_parquet({})", sql_string(path))))
}

pub fn sql_string(path: &Path) -> String {
    let path = normalize_path(path);
    format!("'{}'", path.to_string_lossy().replace('\'', "''"))
}

fn normalize_path(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn normalizes_empty_filter() {
        assert_eq!(normalize_where_clause("").unwrap(), "");
        assert_eq!(normalize_where_clause("   ").unwrap(), "");
    }

    #[test]
    fn normalizes_where_filter() {
        assert_eq!(
            normalize_where_clause("age > 30").unwrap(),
            "WHERE age > 30"
        );
        assert_eq!(
            normalize_where_clause("WHERE paid = true").unwrap(),
            "WHERE paid = true"
        );
        assert_eq!(
            normalize_where_clause("where paid = true").unwrap(),
            "WHERE paid = true"
        );
    }

    #[test]
    fn rejects_unsafe_filter_fragments() {
        assert!(normalize_where_clause("age > 30; DROP TABLE x").is_err());
        assert!(normalize_where_clause("age > 30 -- comment").is_err());
        assert!(normalize_where_clause("age > 30 /* comment */").is_err());
    }

    #[test]
    fn quotes_identifiers() {
        assert_eq!(quote_identifier("plain"), "\"plain\"");
        assert_eq!(quote_identifier("with space"), "\"with space\"");
        assert_eq!(quote_identifier("a\"b"), "\"a\"\"b\"");
    }

    #[test]
    fn builds_preview_projection() {
        let columns = vec!["id".to_string(), "user name".to_string()];
        assert_eq!(
            preview_projection(&columns),
            "CAST(\"id\" AS VARCHAR) AS \"id\", CAST(\"user name\" AS VARCHAR) AS \"user name\""
        );
    }

    #[test]
    fn prepares_advanced_sql() {
        let sql = advanced_sql(Path::new("data's.parquet"), " SELECT * FROM {{file}}; ").unwrap();
        assert_eq!(sql, "SELECT * FROM read_parquet('data''s.parquet')");
    }

    #[test]
    fn validates_advanced_sql() {
        assert!(advanced_sql(Path::new("data.parquet"), "").is_err());
        assert!(advanced_sql(Path::new("data.parquet"), "SELECT 1").is_err());
    }

    #[test]
    fn formats_sql_string_with_fallback_path() {
        let path = PathBuf::from("missing file's.parquet");
        assert_eq!(sql_string(&path), "'missing file''s.parquet'");
    }
}
