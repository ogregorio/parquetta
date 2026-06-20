use crate::duckdb_service::{DuckDBService, ExportFormat, ParquetMetadata, QueryInput, QueryPage};
use gio::prelude::*;
use glib::BoxedAnyObject;
use gtk::prelude::*;
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread;
use std::time::Duration;

const PAGE_SIZE: u64 = 1000;

#[derive(Clone, Debug)]
struct RowData {
    values: Vec<String>,
}

struct AppState {
    sender: Sender<UiMessage>,
    current_file: RefCell<Option<PathBuf>>,
    selected_columns: RefCell<Vec<String>>,
    offset: Cell<u64>,
    next_job_id: Cell<u64>,
    metadata_job_id: Cell<u64>,
    query_job_id: Cell<u64>,
    export_job_id: Cell<u64>,
}

impl AppState {
    fn next_job_id(&self) -> u64 {
        let job_id = self.next_job_id.get().saturating_add(1);
        self.next_job_id.set(job_id);
        job_id
    }
}

enum UiMessage {
    MetadataLoaded {
        job_id: u64,
        path: PathBuf,
        result: Result<ParquetMetadata, String>,
    },
    PageLoaded {
        job_id: u64,
        offset: u64,
        advanced: bool,
        result: Result<QueryPage, String>,
    },
    ExportFinished {
        job_id: u64,
        result: Result<(), String>,
    },
}

#[derive(Clone)]
struct Widgets {
    window: gtk::ApplicationWindow,
    metadata_label: gtk::Label,
    columns_box: gtk::Box,
    filter_entry: gtk::Entry,
    advanced_toggle: gtk::CheckButton,
    apply_filter_button: gtk::Button,
    prev_button: gtk::Button,
    next_button: gtk::Button,
    page_label: gtk::Label,
    table: gtk::ColumnView,
    row_details_revealer: gtk::Revealer,
    row_details_box: gtk::Box,
    status_label: gtk::Label,
}

pub fn build_ui(app: &gtk::Application) {
    if let Err(err) = DuckDBService::new() {
        show_startup_error(app, &format!("Failed to start DuckDB: {err}"));
        return;
    }

    let (sender, receiver) = mpsc::channel();
    let state = Rc::new(AppState {
        sender,
        current_file: RefCell::new(None),
        selected_columns: RefCell::new(Vec::new()),
        offset: Cell::new(0),
        next_job_id: Cell::new(0),
        metadata_job_id: Cell::new(0),
        query_job_id: Cell::new(0),
        export_job_id: Cell::new(0),
    });

    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("Parquetta")
        .default_width(1280)
        .default_height(760)
        .build();

    let open_button = icon_button("document-open-symbolic", "Open Parquet");
    let export_csv_button = icon_button("document-save-symbolic", "Export CSV");
    let export_parquet_button = icon_button("drive-harddisk-symbolic", "Export Parquet");
    let apply_filter_button = icon_button("system-search-symbolic", "Apply filter");

    let filter_entry = gtk::Entry::builder()
        .hexpand(true)
        .width_chars(44)
        .placeholder_text("WHERE age > 30")
        .build();
    let advanced_toggle = gtk::CheckButton::with_label("Advanced");
    let header = gtk::HeaderBar::builder().show_title_buttons(true).build();
    header.pack_start(&open_button);
    header.pack_end(&export_parquet_button);
    header.pack_end(&export_csv_button);
    window.set_titlebar(Some(&header));

    let metadata_label = gtk::Label::builder()
        .xalign(0.0)
        .wrap(true)
        .selectable(true)
        .label("Open a .parquet file to inspect metadata.")
        .build();

    let columns_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
    columns_box.set_vexpand(true);
    let columns_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .min_content_width(280)
        .hexpand(true)
        .vexpand(true)
        .child(&columns_box)
        .build();

    let sidebar = gtk::Box::new(gtk::Orientation::Vertical, 10);
    sidebar.set_hexpand(false);
    sidebar.set_vexpand(true);
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

    let row_details_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
    row_details_box.set_size_request(320, -1);
    row_details_box.set_margin_top(12);
    row_details_box.set_margin_bottom(12);
    row_details_box.set_margin_start(12);
    row_details_box.set_margin_end(12);

    let row_details_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .max_content_width(360)
        .min_content_width(320)
        .hexpand(false)
        .vexpand(true)
        .child(&row_details_box)
        .build();

    let row_details_revealer = gtk::Revealer::builder()
        .transition_type(gtk::RevealerTransitionType::SlideLeft)
        .transition_duration(180)
        .reveal_child(false)
        .child(&row_details_scroll)
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

    let filter_bar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    filter_bar.set_margin_top(8);
    filter_bar.set_margin_bottom(8);
    filter_bar.set_margin_start(12);
    filter_bar.set_margin_end(12);
    filter_bar.append(&advanced_toggle);
    filter_bar.append(&filter_entry);
    filter_bar.append(&apply_filter_button);

    let preview = gtk::Paned::builder()
        .orientation(gtk::Orientation::Horizontal)
        .start_child(&table_scroll)
        .resize_start_child(true)
        .shrink_start_child(false)
        .end_child(&row_details_revealer)
        .resize_end_child(false)
        .shrink_end_child(false)
        .build();

    let content = gtk::Paned::builder()
        .orientation(gtk::Orientation::Horizontal)
        .start_child(&sidebar)
        .resize_start_child(false)
        .shrink_start_child(false)
        .end_child(&preview)
        .resize_end_child(true)
        .shrink_end_child(false)
        .build();
    content.set_position(320);
    preview.set_position(900);

    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.append(&content);
    root.append(&filter_bar);
    root.append(&pager);
    window.set_child(Some(&root));

    let widgets = Widgets {
        window,
        metadata_label,
        columns_box,
        filter_entry,
        advanced_toggle,
        apply_filter_button: apply_filter_button.clone(),
        prev_button,
        next_button,
        page_label,
        table,
        row_details_revealer,
        row_details_box,
        status_label,
    };

    connect_handlers(
        &widgets,
        state,
        receiver,
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
    receiver: Receiver<UiMessage>,
    open_button: gtk::Button,
    apply_filter_button: gtk::Button,
    export_csv_button: gtk::Button,
    export_parquet_button: gtk::Button,
) {
    install_message_pump(widgets, state.clone(), receiver);

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

    let advanced_widgets = widgets.clone();
    let advanced_state = state.clone();
    widgets.advanced_toggle.connect_toggled(move |toggle| {
        advanced_state.offset.set(0);
        advanced_widgets
            .columns_box
            .set_sensitive(!toggle.is_active());
        if toggle.is_active() {
            advanced_widgets
                .filter_entry
                .set_placeholder_text(Some("SELECT * FROM {{file}} LIMIT 1000"));
            advanced_widgets.prev_button.set_sensitive(false);
            advanced_widgets.next_button.set_sensitive(false);
        } else {
            advanced_widgets
                .filter_entry
                .set_placeholder_text(Some("WHERE age > 30"));
        }
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
    state.selected_columns.replace(Vec::new());
    state.offset.set(0);
    clear_column_picker(widgets);
    clear_row_details(widgets);
    set_query_controls_sensitive(widgets, false);
    widgets
        .metadata_label
        .set_label("Loading Parquet metadata...");
    set_status(widgets, "Loading metadata in background...");

    let job_id = state.next_job_id();
    state.metadata_job_id.set(job_id);
    state.query_job_id.set(job_id);
    let sender = state.sender.clone();
    thread::spawn(move || {
        let result = with_service(|service| service.get_metadata(&path));
        let _ = sender.send(UiMessage::MetadataLoaded {
            job_id,
            path,
            result,
        });
    });
}

fn refresh_page(widgets: &Widgets, state: &AppState) {
    let Some(path) = state.current_file.borrow().clone() else {
        set_status(widgets, "Open a .parquet file first.");
        return;
    };

    let input = query_input(widgets, state);
    let offset = state.offset.get();
    let advanced = widgets.advanced_toggle.is_active();
    let job_id = state.next_job_id();
    state.query_job_id.set(job_id);
    set_query_controls_sensitive(widgets, false);
    clear_row_details(widgets);
    set_status(widgets, "Running query in background...");

    let sender = state.sender.clone();
    thread::spawn(move || {
        let result = with_service(|service| service.query_page(&path, PAGE_SIZE, offset, &input));
        let _ = sender.send(UiMessage::PageLoaded {
            job_id,
            offset,
            advanced,
            result,
        });
    });
}

fn install_message_pump(widgets: &Widgets, state: Rc<AppState>, receiver: Receiver<UiMessage>) {
    let widgets = widgets.clone();
    glib::timeout_add_local(Duration::from_millis(50), move || {
        loop {
            match receiver.try_recv() {
                Ok(message) => handle_ui_message(&widgets, &state, message),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return glib::ControlFlow::Break,
            }
        }

        glib::ControlFlow::Continue
    });
}

fn handle_ui_message(widgets: &Widgets, state: &Rc<AppState>, message: UiMessage) {
    match message {
        UiMessage::MetadataLoaded {
            job_id,
            path,
            result,
        } => {
            if state.metadata_job_id.get() != job_id {
                return;
            }

            match result {
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
                    refresh_page(widgets, state);
                }
                Err(err) => {
                    widgets.filter_entry.set_sensitive(true);
                    widgets.advanced_toggle.set_sensitive(true);
                    widgets.apply_filter_button.set_sensitive(true);
                    widgets.prev_button.set_sensitive(false);
                    widgets.next_button.set_sensitive(false);
                    set_status(widgets, &format!("Failed to read metadata: {err}"));
                }
            }
        }
        UiMessage::PageLoaded {
            job_id,
            offset,
            advanced,
            result,
        } => {
            if state.query_job_id.get() != job_id {
                return;
            }

            match result {
                Ok(page) => {
                    render_table(widgets, page.clone());
                    widgets.filter_entry.set_sensitive(true);
                    widgets.advanced_toggle.set_sensitive(true);
                    widgets.apply_filter_button.set_sensitive(true);
                    if advanced {
                        widgets.page_label.set_label("Advanced query");
                        widgets.prev_button.set_sensitive(false);
                        widgets.next_button.set_sensitive(false);
                    } else {
                        let page_number = offset / PAGE_SIZE + 1;
                        widgets.page_label.set_label(&format!("Page {page_number}"));
                        widgets.prev_button.set_sensitive(offset > 0);
                        widgets
                            .next_button
                            .set_sensitive(page.rows.len() as u64 == PAGE_SIZE);
                    }
                    set_status(
                        widgets,
                        &format!("{} rows loaded on this page.", page.rows.len()),
                    );
                }
                Err(err) => {
                    widgets.filter_entry.set_sensitive(true);
                    widgets.advanced_toggle.set_sensitive(true);
                    widgets.apply_filter_button.set_sensitive(true);
                    widgets.prev_button.set_sensitive(!advanced && offset > 0);
                    widgets.next_button.set_sensitive(false);
                    set_status(widgets, &format!("Query failed: {err}"));
                }
            }
        }
        UiMessage::ExportFinished { job_id, result } => {
            if state.export_job_id.get() != job_id {
                return;
            }

            match result {
                Ok(()) => set_status(widgets, "Export completed."),
                Err(err) => set_status(widgets, &format!("Export failed: {err}")),
            }
        }
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
    clear_column_picker(widgets);

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

fn render_table(widgets: &Widgets, page: QueryPage) {
    clear_row_details(widgets);

    while let Some(column) = widgets.table.columns().item(0) {
        let column = column
            .downcast::<gtk::ColumnViewColumn>()
            .expect("ColumnView columns contain ColumnViewColumn");
        widgets.table.remove_column(&column);
    }

    let store = gio::ListStore::new::<BoxedAnyObject>();
    for row in page.rows {
        store.append(&BoxedAnyObject::new(RowData { values: row }));
    }

    let selection = gtk::SingleSelection::new(Some(store));
    selection.set_can_unselect(true);
    selection.set_autoselect(false);

    let detail_widgets = widgets.clone();
    let detail_columns = page.columns.clone();
    selection.connect_selected_item_notify(move |selection| {
        let Some(item) = selection.selected_item().and_downcast::<BoxedAnyObject>() else {
            clear_row_details(&detail_widgets);
            return;
        };

        let row = item.borrow::<RowData>();
        render_row_details(&detail_widgets, &detail_columns, &row.values);
    });

    widgets.table.set_model(Some(&selection));

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

        widgets.table.append_column(
            &gtk::ColumnViewColumn::builder()
                .title(title)
                .factory(&factory)
                .resizable(true)
                .expand(true)
                .build(),
        );
    }
}

fn render_row_details(widgets: &Widgets, columns: &[String], values: &[String]) {
    while let Some(child) = widgets.row_details_box.first_child() {
        widgets.row_details_box.remove(&child);
    }

    let title = gtk::Label::builder()
        .xalign(0.0)
        .label("Row details")
        .build();
    title.add_css_class("heading");
    widgets.row_details_box.append(&title);

    for (index, column) in columns.iter().enumerate() {
        let field = gtk::Box::new(gtk::Orientation::Vertical, 2);
        field.set_margin_bottom(8);

        let name = gtk::Label::builder()
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .label(column)
            .build();
        name.add_css_class("dim-label");

        let value_text = values.get(index).cloned().unwrap_or_default();
        let value = gtk::Label::builder()
            .xalign(0.0)
            .max_width_chars(42)
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::Char)
            .selectable(true)
            .label(&value_text)
            .build();
        value.set_hexpand(true);

        let copy_button = icon_button("edit-copy-symbolic", "Copy value");
        copy_button.add_css_class("flat");
        copy_button.set_valign(gtk::Align::Start);
        copy_button.connect_clicked(move |button| {
            button.clipboard().set_text(&value_text);
        });

        let value_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        value_row.set_hexpand(true);
        value_row.append(&value);
        value_row.append(&copy_button);

        field.append(&name);
        field.append(&value_row);
        widgets.row_details_box.append(&field);
    }

    widgets.row_details_revealer.set_reveal_child(true);
}

fn clear_row_details(widgets: &Widgets) {
    widgets.row_details_revealer.set_reveal_child(false);
    while let Some(child) = widgets.row_details_box.first_child() {
        widgets.row_details_box.remove(&child);
    }
}

fn clear_column_picker(widgets: &Widgets) {
    while let Some(child) = widgets.columns_box.first_child() {
        widgets.columns_box.remove(&child);
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
                let input = query_input(&export_widgets, &state);
                let job_id = state.next_job_id();
                state.export_job_id.set(job_id);
                set_status(&export_widgets, "Exporting in background...");

                let sender = state.sender.clone();
                let source_path = source_path.clone();
                thread::spawn(move || {
                    let result = with_service(|service| {
                        service.export_result(&source_path, &output_path, &input, format)
                    });
                    let _ = sender.send(UiMessage::ExportFinished { job_id, result });
                });
            }
        }
        dialog.destroy();
    });
    dialog.show();
}

fn icon_button(icon_name: &str, tooltip: &str) -> gtk::Button {
    let image = gtk::Image::from_icon_name(icon_name);
    gtk::Button::builder()
        .child(&image)
        .tooltip_text(tooltip)
        .build()
}

fn set_status(widgets: &Widgets, message: &str) {
    widgets.status_label.set_label(message);
}

fn set_query_controls_sensitive(widgets: &Widgets, sensitive: bool) {
    widgets.filter_entry.set_sensitive(sensitive);
    widgets.advanced_toggle.set_sensitive(sensitive);
    widgets.apply_filter_button.set_sensitive(sensitive);
    widgets.prev_button.set_sensitive(sensitive);
    widgets.next_button.set_sensitive(sensitive);
}

fn query_input(widgets: &Widgets, state: &AppState) -> QueryInput {
    if widgets.advanced_toggle.is_active() {
        QueryInput::Advanced {
            sql: widgets.filter_entry.text().to_string(),
        }
    } else {
        QueryInput::Filter {
            selected_columns: state.selected_columns.borrow().clone(),
            where_clause: widgets.filter_entry.text().to_string(),
        }
    }
}

fn with_service<T>(
    operation: impl FnOnce(&DuckDBService) -> Result<T, String>,
) -> Result<T, String> {
    let service = DuckDBService::new()?;
    operation(&service)
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
