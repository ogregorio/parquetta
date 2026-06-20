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
        input: &QueryInput,
    ) -> Result<QueryPage, String> {
        let (columns, sql, params) = match input {
            QueryInput::Filter {
                selected_columns,
                where_clause,
            } => {
                let columns = self.preview_columns(path, selected_columns)?;
                let projected_columns = preview_projection(&columns);
                let filter = normalize_where_clause(where_clause)?;
                let sql = format!(
                    "SELECT {projected_columns} FROM read_parquet({}) {filter} LIMIT ? OFFSET ?",
                    sql_string(path)
                );
                (columns, sql, Some((limit as i64, offset as i64)))
            }
            QueryInput::Advanced { sql } => {
                let sql = advanced_sql(path, sql)?;
                let columns = self.query_columns(&sql)?;
                let sql = format!(
                    "SELECT {} FROM ({sql}) AS parquetta_query",
                    preview_projection(&columns)
                );
                (columns, sql, None)
            }
        };

        let mut stmt = self.conn.prepare(&sql).map_err(|err| err.to_string())?;
        let column_count = columns.len();

        let rows = match params {
            Some((limit, offset)) => stmt
                .query_map(params![limit, offset], |row| row_values(row, column_count))
                .map_err(|err| err.to_string())?
                .collect::<DuckResult<Vec<_>>>()
                .map_err(|err| err.to_string())?,
            None => stmt
                .query_map([], |row| row_values(row, column_count))
                .map_err(|err| err.to_string())?
                .collect::<DuckResult<Vec<_>>>()
                .map_err(|err| err.to_string())?,
        };

        Ok(QueryPage { columns, rows })
    }

    pub fn export_result(
        &self,
        source_path: &Path,
        output_path: &Path,
        input: &QueryInput,
        format: ExportFormat,
    ) -> Result<(), String> {
        let query_sql = match input {
            QueryInput::Filter {
                selected_columns,
                where_clause,
            } => {
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
                format!(
                    "SELECT {projected_columns} FROM read_parquet({}) {filter}",
                    sql_string(source_path)
                )
            }
            QueryInput::Advanced { sql } => advanced_sql(source_path, sql)?,
        };
        let copy_options = match format {
            ExportFormat::Csv => "(FORMAT CSV, HEADER TRUE)",
            ExportFormat::Parquet => "(FORMAT PARQUET)",
        };

        let sql = format!(
            "COPY ({query_sql}) TO {} {copy_options}",
            sql_string(output_path)
        );
        self.conn
            .execute_batch(&sql)
            .map_err(|err| err.to_string())?;
        Ok(())
    }

    fn preview_columns(
        &self,
        path: &Path,
        selected_columns: &[String],
    ) -> Result<Vec<String>, String> {
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

        Ok(columns)
    }

    fn query_columns(&self, sql: &str) -> Result<Vec<String>, String> {
        let describe_sql = format!("DESCRIBE {sql}");
        let mut stmt = self
            .conn
            .prepare(&describe_sql)
            .map_err(|err| err.to_string())?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|err| err.to_string())?;
        let columns = rows
            .collect::<DuckResult<Vec<_>>>()
            .map_err(|err| err.to_string())?;

        if columns.is_empty() {
            return Err("Advanced query did not return any columns".to_string());
        }

        Ok(columns)
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

fn row_values(row: &duckdb::Row<'_>, column_count: usize) -> DuckResult<Vec<String>> {
    let mut values = Vec::with_capacity(column_count);
    for index in 0..column_count {
        values.push(cell_to_string(row, index));
    }
    Ok(values)
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

fn preview_projection(columns: &[String]) -> String {
    columns
        .iter()
        .map(|column| {
            let quoted = quote_identifier(column);
            format!("CAST({quoted} AS VARCHAR) AS {quoted}")
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn advanced_sql(path: &Path, input: &str) -> Result<String, String> {
    let trimmed = input.trim().trim_end_matches(';').trim();
    if trimmed.is_empty() {
        return Err("Advanced query cannot be empty".to_string());
    }
    if !trimmed.contains("{{file}}") {
        return Err("Advanced query must include the {{file}} placeholder".to_string());
    }

    Ok(trimmed.replace("{{file}}", &format!("read_parquet({})", sql_string(path))))
}

fn sql_string(path: &Path) -> String {
    let path = normalize_path(path);
    format!("'{}'", path.to_string_lossy().replace('\'', "''"))
}

fn normalize_path(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}
