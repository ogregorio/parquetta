# Parquetta

Parquetta is a desktop Parquet reader built with Rust + GTK4. It uses DuckDB to
query Parquet files directly with SQL without loading the whole file into memory.

## MVP

- Open `.parquet` files
- Show metadata: rows, columns, types, row groups, and file size
- Paginated preview with `LIMIT`/`OFFSET`
- Simple SQL filter in the form `WHERE age > 30`
- Column selection
- Export the filtered result to CSV or Parquet

## Dependencies

Install Rust, GTK4, and build dependencies. Examples:

```bash
# Fedora
sudo dnf install rust cargo gtk4-devel pkgconf-pkg-config clang

# Ubuntu/Debian
sudo apt install rustc cargo libgtk-4-dev pkg-config clang
```

The `duckdb` crate is configured with the `bundled` and `parquet` features, so
it builds DuckDB with Parquet support as part of the project.

## Run

```bash
cargo run
```

## Architecture

```text
GTK4 App
 ├── File Picker
 ├── Metadata and column sidebar
 ├── SQL filter field
 ├── Gtk.ColumnView preview
 └── DuckDBService
      ├── get_schema()
      ├── get_metadata()
      ├── query_page(limit, offset, columns, where)
      └── export_result()
```
