use crate::query::{
    advanced_sql, normalize_where_clause, preview_projection, quote_identifier, sql_string,
    ExportFormat, QueryInput,
};
use duckdb::{params, Connection, Result as DuckResult};
use std::fs;
use std::path::Path;

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

#[cfg(test)]
mod tests {
    use super::*;
    use duckdb::Connection;
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TempParquet {
        path: PathBuf,
    }

    impl TempParquet {
        fn new() -> Self {
            let mut path = std::env::temp_dir();
            let unique = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock should be after epoch")
                .as_nanos();
            path.push(format!(
                "parquetta-test-{}-{unique}.parquet",
                std::process::id()
            ));

            let sql = format!(
                "COPY (
                    SELECT *
                    FROM (
                        VALUES
                            (1, 'Ada', 37, true, 125.50),
                            (2, 'Grace', 28, false, NULL),
                            (3, 'Linus', 54, true, 300.00)
                    ) AS t(id, name, age, paid, total)
                ) TO {} (FORMAT PARQUET, ROW_GROUP_SIZE 2)",
                crate::query::sql_string(&path)
            );

            Connection::open_in_memory()
                .expect("duckdb connection")
                .execute_batch(&sql)
                .expect("create parquet fixture");

            Self { path }
        }
    }

    impl Drop for TempParquet {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.path);
        }
    }

    #[test]
    fn reads_schema_and_metadata() {
        let fixture = TempParquet::new();
        let service = DuckDBService::new().unwrap();

        let schema = service.get_schema(&fixture.path).unwrap();
        assert_eq!(schema.len(), 5);
        assert_eq!(schema[0].name, "id");
        assert_eq!(schema[0].data_type, "INTEGER");

        let metadata = service.get_metadata(&fixture.path).unwrap();
        assert_eq!(metadata.row_count, Some(3));
        assert_eq!(metadata.columns.len(), 5);
        assert!(metadata.row_groups.unwrap_or_default() >= 1);
        assert!(metadata.file_size > 0);
    }

    #[test]
    fn queries_filtered_pages_with_selected_columns() {
        let fixture = TempParquet::new();
        let service = DuckDBService::new().unwrap();
        let input = QueryInput::Filter {
            selected_columns: vec!["name".to_string(), "total".to_string()],
            where_clause: "age > 30".to_string(),
        };

        let page = service.query_page(&fixture.path, 10, 0, &input).unwrap();

        assert_eq!(page.columns, vec!["name", "total"]);
        assert_eq!(page.rows.len(), 2);
        assert_eq!(page.rows[0], vec!["Ada", "125.50"]);
        assert_eq!(page.rows[1], vec!["Linus", "300.00"]);
    }

    #[test]
    fn paginates_filter_queries() {
        let fixture = TempParquet::new();
        let service = DuckDBService::new().unwrap();
        let input = QueryInput::Filter {
            selected_columns: vec!["id".to_string(), "name".to_string()],
            where_clause: String::new(),
        };

        let first = service.query_page(&fixture.path, 1, 0, &input).unwrap();
        let second = service.query_page(&fixture.path, 1, 1, &input).unwrap();

        assert_eq!(first.rows, vec![vec!["1".to_string(), "Ada".to_string()]]);
        assert_eq!(
            second.rows,
            vec![vec!["2".to_string(), "Grace".to_string()]]
        );
    }

    #[test]
    fn queries_advanced_sql() {
        let fixture = TempParquet::new();
        let service = DuckDBService::new().unwrap();
        let input = QueryInput::Advanced {
            sql: "SELECT paid, count(*) AS count FROM {{file}} GROUP BY paid ORDER BY paid"
                .to_string(),
        };

        let page = service.query_page(&fixture.path, 10, 0, &input).unwrap();

        assert_eq!(page.columns, vec!["paid", "count"]);
        assert_eq!(page.rows, vec![vec!["false", "1"], vec!["true", "2"]]);
    }

    #[test]
    fn validates_bad_queries() {
        let fixture = TempParquet::new();
        let service = DuckDBService::new().unwrap();

        let unsafe_filter = QueryInput::Filter {
            selected_columns: vec!["id".to_string()],
            where_clause: "id > 1; SELECT 1".to_string(),
        };
        assert!(service
            .query_page(&fixture.path, 10, 0, &unsafe_filter)
            .is_err());

        let bad_advanced = QueryInput::Advanced {
            sql: "SELECT 1".to_string(),
        };
        assert!(service
            .query_page(&fixture.path, 10, 0, &bad_advanced)
            .is_err());
    }

    #[test]
    fn exports_filtered_results() {
        let fixture = TempParquet::new();
        let service = DuckDBService::new().unwrap();
        let mut csv_path = std::env::temp_dir();
        csv_path.push(format!("parquetta-export-{}.csv", std::process::id()));
        let _ = fs::remove_file(&csv_path);

        let input = QueryInput::Filter {
            selected_columns: vec!["id".to_string(), "name".to_string()],
            where_clause: "paid = true".to_string(),
        };

        service
            .export_result(&fixture.path, &csv_path, &input, ExportFormat::Csv)
            .unwrap();

        let csv = fs::read_to_string(&csv_path).unwrap();
        assert!(csv.contains("id,name"));
        assert!(csv.contains("1,Ada"));
        assert!(csv.contains("3,Linus"));
        assert!(!csv.contains("Grace"));
        let _ = fs::remove_file(csv_path);
    }
}
