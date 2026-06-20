use duckdb::{params, Connection, Result as DuckResult};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct ColumnInfo {
    pub name: String,
    pub data_type: String,
}

#[derive(Clone, Debug)]
pub struct ParquetMetadata {
    pub row_count: Option<u64>,
    pub columns: Vec<ColumnInfo>,
    pub row_groups: Option<u64>,
    pub file_size: u64,
}

#[derive(Clone, Debug)]
pub struct QueryPage {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExportFormat {
    Csv,
    Parquet,
}

pub struct DuckDBService {
    conn: Connection,
}

impl DuckDBService {
    pub fn new() -> Result<Self, String> {
        Ok(Self {
            conn: Connection::open_in_memory().map_err(|err| err.to_string())?,
        })
    }

    pub fn get_schema(&self, path: &Path) -> Result<Vec<ColumnInfo>, String> {
        let sql = format!(
            "DESCRIBE SELECT * FROM read_parquet({}) LIMIT 0",
            sql_string(path)
        );
        let mut stmt = self.conn.prepare(&sql).map_err(|err| err.to_string())?;
        let rows = stmt
            .query_map([], |row| {
                Ok(ColumnInfo {
                    name: row.get::<_, String>(0)?,
                    data_type: row.get::<_, String>(1)?,
                })
            })
            .map_err(|err| err.to_string())?;

        rows.collect::<DuckResult<Vec<_>>>()
            .map_err(|err| err.to_string())
    }

    pub fn get_metadata(&self, path: &Path) -> Result<ParquetMetadata, String> {
        let columns = self.get_schema(path)?;
        let row_count = self.count_rows(path).ok();
        let row_groups = self.count_row_groups(path).ok();
        let file_size = fs::metadata(path).map(|meta| meta.len()).unwrap_or(0);

        Ok(ParquetMetadata {
            row_count,
            columns,
            row_groups,
            file_size,
        })
    }

    pub fn query_page(
        &self,
        path: &Path,
        limit: u64,
        offset: u64,
        selected_columns: &[String],
        where_clause: &str,
    ) -> Result<QueryPage, String> {
        let projected_columns = self.preview_projection(path, selected_columns)?;
        let filter = normalize_where_clause(where_clause)?;
        let sql = format!(
            "SELECT {projected_columns} FROM read_parquet({}) {filter} LIMIT ? OFFSET ?",
            sql_string(path)
        );

        let mut stmt = self.conn.prepare(&sql).map_err(|err| err.to_string())?;
        let column_names = stmt
            .column_names()
            .iter()
            .map(|name| name.to_string())
            .collect::<Vec<_>>();
        let column_count = column_names.len();

        let rows = stmt
            .query_map(params![limit as i64, offset as i64], |row| {
                let mut values = Vec::with_capacity(column_count);
                for index in 0..column_count {
                    values.push(cell_to_string(row, index));
                }
                Ok(values)
            })
            .map_err(|err| err.to_string())?;

        Ok(QueryPage {
            columns: column_names,
            rows: rows
                .collect::<DuckResult<Vec<_>>>()
                .map_err(|err| err.to_string())?,
        })
    }

    pub fn export_result(
        &self,
        source_path: &Path,
        output_path: &Path,
        selected_columns: &[String],
        where_clause: &str,
        format: ExportFormat,
    ) -> Result<(), String> {
        let projected_columns = if selected_columns.is_empty() {
            "*".to_string()
        } else {
            selected_columns
                .iter()
                .map(|column| quote_identifier(column))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let filter = normalize_where_clause(where_clause)?;
        let copy_options = match format {
            ExportFormat::Csv => "(FORMAT CSV, HEADER TRUE)",
            ExportFormat::Parquet => "(FORMAT PARQUET)",
        };

        let sql = format!(
            "COPY (SELECT {projected_columns} FROM read_parquet({}) {filter}) TO {} {copy_options}",
            sql_string(source_path),
            sql_string(output_path)
        );
        self.conn
            .execute_batch(&sql)
            .map_err(|err| err.to_string())?;
        Ok(())
    }

    fn preview_projection(
        &self,
        path: &Path,
        selected_columns: &[String],
    ) -> Result<String, String> {
        let columns = if selected_columns.is_empty() {
            self.get_schema(path)?
                .into_iter()
                .map(|column| column.name)
                .collect::<Vec<_>>()
        } else {
            selected_columns.to_vec()
        };

        if columns.is_empty() {
            return Err("No columns found in the Parquet file".to_string());
        }

        Ok(columns
            .iter()
            .map(|column| {
                let quoted = quote_identifier(column);
                format!("CAST({quoted} AS VARCHAR) AS {quoted}")
            })
            .collect::<Vec<_>>()
            .join(", "))
    }

    fn count_rows(&self, path: &Path) -> DuckResult<u64> {
        let sql = format!("SELECT count(*) FROM read_parquet({})", sql_string(path));
        self.conn.query_row(&sql, [], |row| row.get::<_, u64>(0))
    }

    fn count_row_groups(&self, path: &Path) -> DuckResult<u64> {
        let sql = format!(
            "SELECT count(DISTINCT row_group_id) FROM parquet_metadata({})",
            sql_string(path)
        );
        self.conn.query_row(&sql, [], |row| row.get::<_, u64>(0))
    }
}

fn cell_to_string(row: &duckdb::Row<'_>, index: usize) -> String {
    row.get::<_, Option<String>>(index)
        .ok()
        .flatten()
        .unwrap_or_else(|| "NULL".to_string())
}

fn normalize_where_clause(input: &str) -> Result<String, String> {
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

fn quote_identifier(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}

fn sql_string(path: &Path) -> String {
    let path = normalize_path(path);
    format!("'{}'", path.to_string_lossy().replace('\'', "''"))
}

fn normalize_path(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}
