use crate::duckdb_service::{DuckDBService, ExportFormat, ParquetMetadata, QueryPage};
use gio::prelude::*;
use glib::BoxedAnyObject;
use gtk::prelude::*;
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;

const PAGE_SIZE: u64 = 1000;

#[derive(Clone, Debug)]
struct RowData {
    values: Vec<String>,
}

struct AppState {
    service: DuckDBService,
    current_file: RefCell<Option<PathBuf>>,
    selected_columns: RefCell<Vec<String>>,
    offset: Cell<u64>,
}

#[derive(Clone)]
struct Widgets {
    window: gtk::ApplicationWindow,
    metadata_label: gtk::Label,
    columns_box: gtk::Box,
    filter_entry: gtk::Entry,
    prev_button: gtk::Button,
    next_button: gtk::Button,
    page_label: gtk::Label,
    table: gtk::ColumnView,
    status_label: gtk::Label,
}

pub fn build_ui(app: &gtk::Application) {
    let state = match DuckDBService::new() {
        Ok(service) => Rc::new(AppState {
            service,
            current_file: RefCell::new(None),
            selected_columns: RefCell::new(Vec::new()),
            offset: Cell::new(0),
        }),
        Err(err) => {
            show_startup_error(app, &format!("Failed to start DuckDB: {err}"));
            return;
        }
    };

    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("Parquetta")
        .default_width(1280)
        .default_height(760)
        .build();

    let open_button = icon_button("document-open-symbolic", "Open Parquet");
    let export_csv_button = icon_button("document-save-symbolic", "Export CSV");
    let export_parquet_button = icon_button("drive-harddisk-symbolic", "Export Parquet");
    let apply_filter_button = gtk::Button::with_label("Apply");

    let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    header.set_margin_top(10);
    header.set_margin_bottom(10);
    header.set_margin_start(12);
    header.set_margin_end(12);
    header.append(&open_button);
    header.append(&gtk::Separator::new(gtk::Orientation::Vertical));

    let filter_entry = gtk::Entry::builder()
        .hexpand(true)
        .placeholder_text("WHERE age > 30")
        .build();
    header.append(&filter_entry);
    header.append(&apply_filter_button);
    header.append(&export_csv_button);
    header.append(&export_parquet_button);

    let metadata_label = gtk::Label::builder()
        .xalign(0.0)
        .wrap(true)
        .selectable(true)
        .label("Open a .parquet file to inspect metadata.")
        .build();

    let columns_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
    let columns_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .min_content_width(280)
        .child(&columns_box)
        .build();

    let sidebar = gtk::Box::new(gtk::Orientation::Vertical, 10);
    sidebar.set_margin_top(12);
    sidebar.set_margin_bottom(12);
    sidebar.set_margin_start(12);
    sidebar.set_margin_end(12);
    sidebar.append(&metadata_label);
    sidebar.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    sidebar.append(&gtk::Label::builder().xalign(0.0).label("Columns").build());
    sidebar.append(&columns_scroll);

    let table = gtk::ColumnView::builder()
        .hexpand(true)
        .vexpand(true)
        .show_column_separators(true)
        .show_row_separators(true)
        .build();
    let table_scroll = gtk::ScrolledWindow::builder()
        .hexpand(true)
        .vexpand(true)
        .child(&table)
        .build();

    let prev_button = gtk::Button::with_label("Previous");
    let next_button = gtk::Button::with_label("Next");
    let page_label = gtk::Label::new(Some("Page 1"));
    let status_label = gtk::Label::builder()
        .xalign(0.0)
        .hexpand(true)
        .label("No file opened.")
        .build();

    let pager = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    pager.set_margin_top(8);
    pager.set_margin_bottom(8);
    pager.set_margin_start(12);
    pager.set_margin_end(12);
    pager.append(&prev_button);
    pager.append(&next_button);
    pager.append(&page_label);
    pager.append(&status_label);

    let content = gtk::Paned::builder()
        .orientation(gtk::Orientation::Horizontal)
        .start_child(&sidebar)
        .resize_start_child(false)
        .shrink_start_child(false)
        .end_child(&table_scroll)
        .build();

    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.append(&header);
    root.append(&content);
    root.append(&pager);
    window.set_child(Some(&root));

    let widgets = Widgets {
        window,
        metadata_label,
        columns_box,
        filter_entry,
        prev_button,
        next_button,
        page_label,
        table,
        status_label,
    };

    connect_handlers(
        &widgets,
        state,
        open_button,
        apply_filter_button,
        export_csv_button,
        export_parquet_button,
    );
    widgets.window.present();
}

fn connect_handlers(
    widgets: &Widgets,
    state: Rc<AppState>,
    open_button: gtk::Button,
    apply_filter_button: gtk::Button,
    export_csv_button: gtk::Button,
    export_parquet_button: gtk::Button,
) {
    let open_widgets = widgets.clone();
    let open_state = state.clone();
    open_button.connect_clicked(move |_| {
        open_file_dialog(&open_widgets.window, {
            let widgets = open_widgets.clone();
            let state = open_state.clone();
            move |path| load_file(&widgets, state.clone(), path)
        });
    });

    let apply_widgets = widgets.clone();
    let apply_state = state.clone();
    apply_filter_button.connect_clicked(move |_| {
        apply_state.offset.set(0);
        refresh_page(&apply_widgets, &apply_state);
    });

    let filter_widgets = widgets.clone();
    let filter_state = state.clone();
    widgets.filter_entry.connect_activate(move |_| {
        filter_state.offset.set(0);
        refresh_page(&filter_widgets, &filter_state);
    });

    let prev_widgets = widgets.clone();
    let prev_state = state.clone();
    widgets.prev_button.connect_clicked(move |_| {
        let offset = prev_state.offset.get();
        prev_state.offset.set(offset.saturating_sub(PAGE_SIZE));
        refresh_page(&prev_widgets, &prev_state);
    });

    let next_widgets = widgets.clone();
    let next_state = state.clone();
    widgets.next_button.connect_clicked(move |_| {
        next_state.offset.set(next_state.offset.get() + PAGE_SIZE);
        refresh_page(&next_widgets, &next_state);
    });

    let csv_widgets = widgets.clone();
    let csv_state = state.clone();
    export_csv_button.connect_clicked(move |_| {
        export_dialog(&csv_widgets, csv_state.clone(), ExportFormat::Csv);
    });

    let parquet_widgets = widgets.clone();
    let parquet_state = state;
    export_parquet_button.connect_clicked(move |_| {
        export_dialog(
            &parquet_widgets,
            parquet_state.clone(),
            ExportFormat::Parquet,
        );
    });
}

fn load_file(widgets: &Widgets, state: Rc<AppState>, path: PathBuf) {
    state.current_file.replace(Some(path.clone()));
    state.offset.set(0);

    match state.service.get_metadata(&path) {
        Ok(metadata) => {
            state.selected_columns.replace(
                metadata
                    .columns
                    .iter()
                    .map(|column| column.name.clone())
                    .collect(),
            );
            render_metadata(widgets, &path, &metadata);
            render_column_picker(widgets, state.clone(), &metadata);
            refresh_page(widgets, &state);
        }
        Err(err) => set_status(widgets, &format!("Failed to read metadata: {err}")),
    }
}

fn refresh_page(widgets: &Widgets, state: &AppState) {
    let Some(path) = state.current_file.borrow().clone() else {
        set_status(widgets, "Open a .parquet file first.");
        return;
    };

    let columns = state.selected_columns.borrow().clone();
    let where_clause = widgets.filter_entry.text().to_string();
    match state.service.query_page(
        &path,
        PAGE_SIZE,
        state.offset.get(),
        &columns,
        &where_clause,
    ) {
        Ok(page) => {
            render_table(&widgets.table, page.clone());
            let page_number = state.offset.get() / PAGE_SIZE + 1;
            widgets.page_label.set_label(&format!("Page {page_number}"));
            widgets.prev_button.set_sensitive(state.offset.get() > 0);
            widgets
                .next_button
                .set_sensitive(page.rows.len() as u64 == PAGE_SIZE);
            set_status(
                widgets,
                &format!("{} rows loaded on this page.", page.rows.len()),
            );
        }
        Err(err) => set_status(widgets, &format!("Query failed: {err}")),
    }
}

fn render_metadata(widgets: &Widgets, path: &Path, metadata: &ParquetMetadata) {
    let row_count = metadata
        .row_count
        .map(|value| value.to_string())
        .unwrap_or_else(|| "unknown".to_string());
    let row_groups = metadata
        .row_groups
        .map(|value| value.to_string())
        .unwrap_or_else(|| "unknown".to_string());

    widgets.metadata_label.set_label(&format!(
        "{}\n\nRows: {row_count}\nColumns: {}\nRow groups: {row_groups}\nSize: {}",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("file.parquet"),
        metadata.columns.len(),
        human_size(metadata.file_size)
    ));
}

fn render_column_picker(widgets: &Widgets, state: Rc<AppState>, metadata: &ParquetMetadata) {
    while let Some(child) = widgets.columns_box.first_child() {
        widgets.columns_box.remove(&child);
    }

    for column in &metadata.columns {
        let check = gtk::CheckButton::with_label(&format!("{}: {}", column.name, column.data_type));
        check.set_active(true);

        let column_name = column.name.clone();
        let picker_widgets = widgets.clone();
        let picker_state = state.clone();
        check.connect_toggled(move |button| {
            let mut selected = picker_state.selected_columns.borrow_mut();
            if button.is_active() {
                if !selected.iter().any(|name| name == &column_name) {
                    selected.push(column_name.clone());
                }
            } else {
                selected.retain(|name| name != &column_name);
            }
            picker_state.offset.set(0);
            drop(selected);
            refresh_page(&picker_widgets, &picker_state);
        });

        widgets.columns_box.append(&check);
    }
}

fn render_table(table: &gtk::ColumnView, page: QueryPage) {
    while let Some(column) = table.columns().item(0) {
        let column = column
            .downcast::<gtk::ColumnViewColumn>()
            .expect("ColumnView columns contain ColumnViewColumn");
        table.remove_column(&column);
    }

    let store = gio::ListStore::new::<BoxedAnyObject>();
    for row in page.rows {
        store.append(&BoxedAnyObject::new(RowData { values: row }));
    }

    let selection = gtk::NoSelection::new(Some(store));
    table.set_model(Some(&selection));

    for (index, title) in page.columns.iter().enumerate() {
        let factory = gtk::SignalListItemFactory::new();
        factory.connect_setup(|_, list_item| {
            let label = gtk::Label::builder()
                .xalign(0.0)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .single_line_mode(true)
                .width_chars(18)
                .build();
            list_item.set_child(Some(&label));
        });
        factory.connect_bind(move |_, list_item| {
            let Some(item) = list_item.item().and_downcast::<BoxedAnyObject>() else {
                return;
            };
            let Some(label) = list_item.child().and_downcast::<gtk::Label>() else {
                return;
            };
            let row = item.borrow::<RowData>();
            label.set_label(row.values.get(index).map(String::as_str).unwrap_or(""));
        });

        table.append_column(
            &gtk::ColumnViewColumn::builder()
                .title(title)
                .factory(&factory)
                .resizable(true)
                .expand(true)
                .build(),
        );
    }
}

fn open_file_dialog<F>(window: &gtk::ApplicationWindow, on_file: F)
where
    F: Fn(PathBuf) + 'static,
{
    let dialog = gtk::FileChooserNative::builder()
        .title("Open Parquet")
        .transient_for(window)
        .action(gtk::FileChooserAction::Open)
        .accept_label("Open")
        .cancel_label("Cancel")
        .build();
    let filter = gtk::FileFilter::new();
    filter.set_name(Some("Parquet"));
    filter.add_pattern("*.parquet");
    dialog.add_filter(&filter);

    dialog.connect_response(move |dialog, response| {
        if response == gtk::ResponseType::Accept {
            if let Some(path) = dialog.file().and_then(|file| file.path()) {
                on_file(path);
            }
        }
        dialog.destroy();
    });
    dialog.show();
}

fn export_dialog(widgets: &Widgets, state: Rc<AppState>, format: ExportFormat) {
    let Some(source_path) = state.current_file.borrow().clone() else {
        set_status(widgets, "Open a file before exporting.");
        return;
    };

    let dialog = gtk::FileChooserNative::builder()
        .title("Export result")
        .transient_for(&widgets.window)
        .action(gtk::FileChooserAction::Save)
        .accept_label("Export")
        .cancel_label("Cancel")
        .build();
    dialog.set_current_name(match format {
        ExportFormat::Csv => "result.csv",
        ExportFormat::Parquet => "result.parquet",
    });

    let export_widgets = widgets.clone();
    dialog.connect_response(move |dialog, response| {
        if response == gtk::ResponseType::Accept {
            if let Some(output_path) = dialog.file().and_then(|file| file.path()) {
                let selected_columns = state.selected_columns.borrow().clone();
                let where_clause = export_widgets.filter_entry.text().to_string();
                match state.service.export_result(
                    &source_path,
                    &output_path,
                    &selected_columns,
                    &where_clause,
                    format,
                ) {
                    Ok(()) => set_status(&export_widgets, "Export completed."),
                    Err(err) => set_status(&export_widgets, &format!("Export failed: {err}")),
                }
            }
        }
        dialog.destroy();
    });
    dialog.show();
}

fn icon_button(icon_name: &str, tooltip: &str) -> gtk::Button {
    let image = gtk::Image::from_icon_name(icon_name);
    let button = gtk::Button::builder()
        .child(&image)
        .tooltip_text(tooltip)
        .build();
    button
}

fn set_status(widgets: &Widgets, message: &str) {
    widgets.status_label.set_label(message);
}

fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    format!("{size:.1} {}", UNITS[unit])
}

fn show_startup_error(app: &gtk::Application, message: &str) {
    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("Parquetta")
        .default_width(520)
        .default_height(160)
        .build();
    let label = gtk::Label::builder()
        .margin_top(24)
        .margin_bottom(24)
        .margin_start(24)
        .margin_end(24)
        .wrap(true)
        .label(message)
        .build();
    window.set_child(Some(&label));
    window.present();
}
