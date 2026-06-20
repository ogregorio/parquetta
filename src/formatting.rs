pub const PAGE_SIZE: u64 = 1000;
pub const MIN_INITIAL_COLUMN_WIDTH: i32 = 140;
pub const FALLBACK_TABLE_WIDTH: i32 = 960;

pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    format!("{size:.1} {}", UNITS[unit])
}

pub fn initial_column_width(table_width: i32, column_count: usize) -> i32 {
    let column_count = i32::try_from(column_count).unwrap_or(i32::MAX).max(1);
    let table_width = table_width.max(FALLBACK_TABLE_WIDTH);

    (table_width / column_count).max(MIN_INITIAL_COLUMN_WIDTH)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_byte_sizes() {
        assert_eq!(human_size(0), "0.0 B");
        assert_eq!(human_size(512), "512.0 B");
        assert_eq!(human_size(1024), "1.0 KB");
        assert_eq!(human_size(1536), "1.5 KB");
        assert_eq!(human_size(1024 * 1024 * 3), "3.0 MB");
        assert_eq!(human_size(1024_u64.pow(4) * 2), "2.0 TB");
    }

    #[test]
    fn calculates_initial_column_width() {
        assert_eq!(initial_column_width(1200, 3), 400);
        assert_eq!(initial_column_width(300, 3), 320);
        assert_eq!(initial_column_width(960, 20), MIN_INITIAL_COLUMN_WIDTH);
        assert_eq!(initial_column_width(960, 0), 960);
    }
}
