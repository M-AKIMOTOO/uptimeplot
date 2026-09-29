#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use chrono::{Datelike, Duration, NaiveDate, TimeZone, Timelike, Utc};
use eframe::egui;
use egui_plot::{Bar, BarChart, Corner, GridMark, Legend, Line, Plot, PlotPoints, Points};
use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

mod utils;

const PLOT_Y_AXIS_MIN_WIDTH: f32 = 96.0;
const PLOT_MIN_ELEVATION_DEG: f64 = 1.0;
const DEFAULT_FIVE_POINT_OFFSET_ARCMIN: f64 = 2.0;
const ANTENNA_FIXED_AZ_DEG: f64 = 244.0;
const ANTENNA_FIXED_EL_DEG: f64 = 20.0;
const SKD_HISTORY_LIMIT: usize = 100;

const SKD_COL_NUM: f32 = 24.0;
const SKD_COL_SOURCE: f32 = 116.0;
const SKD_COL_DATE: f32 = 90.0;
const SKD_COL_TIME: f32 = 110.0;
const SKD_COL_DURATION: f32 = 80.0;
const SKD_COL_AZEL: f32 = 104.0;
const SKD_COL_ANTENNA: f32 = 220.0;
const SKD_COL_DELETE: f32 = 44.0;
const SKD_TABLE_MIN_WIDTH: f32 = SKD_COL_NUM
    + SKD_COL_SOURCE
    + SKD_COL_DATE
    + SKD_COL_TIME
    + SKD_COL_DURATION
    + SKD_COL_AZEL * 2.0
    + SKD_COL_ANTENNA * 2.0
    + SKD_COL_DELETE
    + 80.0;

#[derive(PartialEq, Clone, Copy)]
enum AppTab {
    UptimePlotters,
    Parameters,
    PolarPlot,
    LstPlot,
    SkdTable,
}

#[derive(Clone, Copy)]
enum OutputTarget {
    UtAzel,
    Polar,
    Lst,
}

struct OutputCaptureState {
    targets: Vec<OutputTarget>,
    index: usize,
    previous_tab: AppTab,
    screenshot_requested: bool,
}

#[derive(Debug, Default)]
struct CliArgs {
    station_path: Option<PathBuf>,
    source_path: Option<PathBuf>,
    drg_path: Option<PathBuf>,
    terminal: bool,
}

impl CliArgs {
    fn parse() -> Self {
        let mut parsed = Self::default();
        let mut args = std::env::args_os().skip(1);

        while let Some(arg) = args.next() {
            let option = arg.to_string_lossy();
            match option.as_ref() {
                "-h" | "--help" => {
                    println!(
                        "Uptime Plotter {}\n\nUsage: uptimeplot [OPTIONS]\n\nOptions:\n  --drg <FILE>           Load a DRG schedule\n  --terminal             Print the DRG schedule check as TSV without a GUI\n  --station-path <PATH>  Path to station.txt\n  --source-path <PATH>   Path to source.txt\n  -h, --help             Print help\n  -V, --version          Print version",
                        env!("CARGO_PKG_VERSION")
                    );
                    std::process::exit(0);
                }
                "-V" | "--version" => {
                    println!("uptimeplot {}", env!("CARGO_PKG_VERSION"));
                    std::process::exit(0);
                }
                "--station-path" => {
                    parsed.station_path = Some(Self::next_path(&mut args, "--station-path"));
                }
                "--source-path" => {
                    parsed.source_path = Some(Self::next_path(&mut args, "--source-path"));
                }
                "--drg" => {
                    parsed.drg_path = Some(Self::next_path(&mut args, "--drg"));
                }
                "--terminal" => {
                    parsed.terminal = true;
                }
                _ => {
                    if let Some(path) = option.strip_prefix("--station-path=") {
                        parsed.station_path = Some(PathBuf::from(path));
                    } else if let Some(path) = option.strip_prefix("--source-path=") {
                        parsed.source_path = Some(PathBuf::from(path));
                    } else if let Some(path) = option.strip_prefix("--drg=") {
                        parsed.drg_path = Some(PathBuf::from(path));
                    } else {
                        eprintln!("Unknown option: {option}\nUse --help for usage.");
                        std::process::exit(2);
                    }
                }
            }
        }

        parsed
    }

    fn next_path(args: &mut impl Iterator<Item = std::ffi::OsString>, option: &str) -> PathBuf {
        match args.next() {
            Some(path) => PathBuf::from(path),
            None => {
                eprintln!("Missing value for {option}\nUse --help for usage.");
                std::process::exit(2);
            }
        }
    }
}

fn main() -> Result<(), eframe::Error> {
    let cli_args = CliArgs::parse();

    if cli_args.terminal {
        let Some(drg_path) = cli_args.drg_path.as_deref() else {
            eprintln!("uptimeplot: --terminal requires --drg <FILE>");
            std::process::exit(2);
        };
        match print_terminal_drg_report(&cli_args, drg_path) {
            Ok(true) => return Ok(()),
            Ok(false) => std::process::exit(1),
            Err(error) => {
                eprintln!("uptimeplot: {error}");
                std::process::exit(2);
            }
        }
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1280.0, 720.0]),
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    eframe::run_native(
        "Uptime Plotter",
        options,
        Box::new(move |cc| {
            // Use move to capture cli_args
            let app = Box::new(UptimePlotApp::new(cli_args)); // Call new constructor

            // Increase font size
            let mut style = (*cc.egui_ctx.global_style()).clone();
            for (_text_style, font_id) in style.text_styles.iter_mut() {
                font_id.size *= 1.5; // Increase by 1.5 times
            }
            style.visuals.panel_fill = egui::Color32::TRANSPARENT;
            cc.egui_ctx.set_global_style(style);

            Ok(app)
        }),
    )
}

fn terminal_float(value: Option<f64>) -> String {
    value
        .filter(|number| number.is_finite())
        .map(|number| format!("{number:.3}"))
        .unwrap_or_else(|| "NaN".to_string())
}

fn print_terminal_drg_report(cli_args: &CliArgs, drg_path: &Path) -> Result<bool, String> {
    let mut app = UptimePlotApp::new(CliArgs {
        station_path: cli_args.station_path.clone(),
        source_path: cli_args.source_path.clone(),
        drg_path: None,
        terminal: false,
    });
    if app.antennas.is_empty() {
        return Err(format!("No antennas loaded from {}", app.antenna_file_path));
    }

    app.input_drg_file_path = drg_path.to_string_lossy().to_string();
    app.load_drg_file()?;
    if app.skd_rows.is_empty() {
        return Err("The DRG contains no SKED rows.".to_string());
    }

    let mut antenna_indices = Vec::new();
    for index in [app.selected_antenna, app.selected_antenna_2] {
        if index < app.antennas.len() && !antenna_indices.contains(&index) {
            antenna_indices.push(index);
        }
    }
    if antenna_indices.is_empty() {
        return Err("No antenna is available for schedule checks.".to_string());
    }

    let source_map: HashMap<&str, &Source> = app
        .sources
        .iter()
        .map(|(source, _)| (source.name.as_str(), source))
        .collect();
    let mut previous_ends: Vec<Option<ScanEnd>> = vec![None; antenna_indices.len()];
    let mut failures = 0_usize;

    println!(
        "scan\tsource\tstart_utc\tduration_s\tantenna\tstart_az_deg\tstart_el_deg\tend_az_deg\tend_el_deg\taz_rate_deg_min\tel_rate_deg_min\taz_min_deg\taz_max_deg\tel_min_deg\tel_max_deg\tgeometry_ok\tlimits_ok\thas_previous\tslew_required_s\tslew_available_s\tslew_ok\tscan_ok"
    );

    for (scan_index, row) in app.skd_rows.iter().enumerate() {
        let start_dt = schedule_datetime(row.start_date, &row.start_time)?;
        let start_text = start_dt.format("%Y-%m-%dT%H:%M:%S").to_string();
        let source = source_map.get(row.source_name.as_str()).copied();

        for (output_index, antenna_index) in antenna_indices.iter().copied().enumerate() {
            let antenna = &app.antennas[antenna_index];
            let previous_end = previous_ends[output_index];
            let mut start_az = None;
            let mut start_el = None;
            let mut end_az = None;
            let mut end_el = None;
            let mut geometry_ok = false;
            let mut limits_ok = false;
            let mut has_previous = false;
            let mut slew_required = None;
            let mut slew_available = None;
            let mut slew_ok = false;

            if let Some(source) = source {
                if let (
                    Some((_, current_start_az, current_start_el)),
                    Some((end_dt, current_end_az, current_end_el)),
                ) = (
                    scan_az_el_for(row, source, antenna.pos, false),
                    scan_az_el_for(row, source, antenna.pos, true),
                ) {
                    geometry_ok = true;
                    start_az = Some(current_start_az);
                    start_el = Some(current_start_el);
                    end_az = Some(current_end_az);
                    end_el = Some(current_end_el);
                    limits_ok = antenna.allows(current_start_az, current_start_el)
                        && antenna.allows(current_end_az, current_end_el);

                    if let Some((previous_end_dt, previous_end_az, previous_end_el)) = previous_end
                    {
                        has_previous = true;
                        let available = (start_dt - previous_end_dt).num_seconds() as f64;
                        let required = antenna.slew_seconds(
                            previous_end_az,
                            previous_end_el,
                            current_start_az,
                            current_start_el,
                        );
                        slew_available = Some(available);
                        slew_required = required;
                        slew_ok = available >= 0.0
                            && required
                                .map(|required| available + 1.0e-6 >= required)
                                .unwrap_or(false);
                    } else {
                        slew_ok = true;
                    }

                    previous_ends[output_index] = Some((end_dt, current_end_az, current_end_el));
                } else {
                    previous_ends[output_index] = None;
                }
            } else {
                previous_ends[output_index] = None;
            }

            let scan_ok = geometry_ok && limits_ok && slew_ok;
            if !scan_ok {
                failures += 1;
            }
            println!(
                "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{:.3}\t{:.3}\t{:.3}\t{:.3}\t{:.3}\t{:.3}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                scan_index + 1,
                row.source_name,
                start_text,
                row.duration_sec,
                antenna.name,
                terminal_float(start_az),
                terminal_float(start_el),
                terminal_float(end_az),
                terminal_float(end_el),
                antenna.az_rate_deg_per_min,
                antenna.el_rate_deg_per_min,
                antenna.az_min_deg,
                antenna.az_max_deg,
                antenna.el_min_deg,
                antenna.el_max_deg,
                geometry_ok as u8,
                limits_ok as u8,
                has_previous as u8,
                terminal_float(slew_required),
                terminal_float(slew_available),
                slew_ok as u8,
                scan_ok as u8,
            );
        }
    }

    eprintln!(
        "# drg={} scans={} antennas={} failures={}",
        output_drg_path(&drg_path.to_string_lossy())?.display(),
        app.skd_rows.len(),
        antenna_indices.len(),
        failures
    );
    Ok(failures == 0)
}

struct Station {
    name: String,
    pos: [f64; 3],
}

#[derive(Clone)]
struct Antenna {
    code: String,
    name: String,
    pos: [f64; 3],
    az_rate_deg_per_min: f64,
    az_min_deg: f64,
    az_max_deg: f64,
    el_rate_deg_per_min: f64,
    el_min_deg: f64,
    el_max_deg: f64,
}

#[derive(Clone)]
struct Source {
    name: String,
    ra_rad: f64,
    dec_rad: f64,
    ra_h: i32,
    ra_m: i32,
    ra_s: f64,
    dec_sign: char,
    dec_d: i32,
    dec_m: i32,
    dec_s: f64,
    epoch: String,
}

#[derive(Clone, PartialEq)]
struct SkdRow {
    source_name: String,
    start_date: NaiveDate,
    start_time: String,
    duration_sec: u32,
    az_offset_deg: f64,
    el_offset_deg: f64,
    ra_offset_deg: f64,
    dec_offset_deg: f64,
    include_station_offsets: bool,
}

#[derive(Clone)]
struct SkdRowStatus {
    start_geometry: String,
    end_geometry: String,
    motion_1: String,
    motion_2: String,
}

impl Antenna {
    fn allows(&self, az_deg: f64, el_deg: f64) -> bool {
        self.az_in_limit(az_deg).is_some() && el_deg >= self.el_min_deg && el_deg <= self.el_max_deg
    }

    fn az_in_limit(&self, az_deg: f64) -> Option<f64> {
        (-2..=2)
            .map(|turn| az_deg + 360.0 * turn as f64)
            .find(|az| *az >= self.az_min_deg && *az <= self.az_max_deg)
    }

    fn az_for_slew(&self, az_deg: f64, reference: Option<f64>) -> Option<f64> {
        let mut candidates: Vec<f64> = (-2..=2)
            .map(|turn| az_deg + 360.0 * turn as f64)
            .filter(|az| *az >= self.az_min_deg && *az <= self.az_max_deg)
            .collect();
        if candidates.is_empty() {
            return None;
        }
        if let Some(reference) = reference {
            candidates.sort_by(|a, b| {
                (a - reference)
                    .abs()
                    .partial_cmp(&(b - reference).abs())
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        }
        candidates.first().copied()
    }

    fn slew_seconds(&self, from_az: f64, from_el: f64, to_az: f64, to_el: f64) -> Option<f64> {
        if self.az_rate_deg_per_min <= 0.0 || self.el_rate_deg_per_min <= 0.0 {
            return None;
        }
        if from_el < self.el_min_deg
            || from_el > self.el_max_deg
            || to_el < self.el_min_deg
            || to_el > self.el_max_deg
        {
            return None;
        }
        let from_az = self.az_for_slew(from_az, None)?;
        let to_az = self.az_for_slew(to_az, Some(from_az))?;
        let az_sec = (to_az - from_az).abs() / self.az_rate_deg_per_min * 60.0 + 5.0;
        let el_sec = (to_el - from_el).abs() / self.el_rate_deg_per_min * 60.0 + 5.0;
        Some(az_sec.max(el_sec) + 2.0)
    }
}

type ScanEnd = (chrono::NaiveDateTime, f64, f64);

fn antenna_motion_status(
    row: &SkdRow,
    source: &Source,
    antenna: &Antenna,
    prev_end: Option<ScanEnd>,
) -> (String, Option<ScanEnd>) {
    match (
        scan_az_el_for(row, source, antenna.pos, false),
        scan_az_el_for(row, source, antenna.pos, true),
    ) {
        (Some((start_dt, start_az, start_el)), Some((end_dt, end_az, end_el))) => {
            let limit_text = if antenna.allows(start_az, start_el) && antenna.allows(end_az, end_el)
            {
                "OK"
            } else {
                "NO"
            };

            let motion = if let Some((prev_end_dt, prev_end_az, prev_end_el)) = prev_end {
                let gap_sec = (start_dt - prev_end_dt).num_seconds();
                if gap_sec < 0 {
                    format!("{} NO", limit_text)
                } else {
                    match antenna.slew_seconds(prev_end_az, prev_end_el, start_az, start_el) {
                        Some(need_sec) if (gap_sec as f64) + 1.0e-6 >= need_sec => {
                            format!("{} OK {:.0}/{:.0}s", limit_text, need_sec.ceil(), gap_sec)
                        }
                        Some(need_sec) => {
                            format!("{} NO {:.0}/{:.0}s", limit_text, need_sec.ceil(), gap_sec)
                        }
                        None => format!("{} NO", limit_text),
                    }
                }
            } else {
                limit_text.to_string()
            };
            (motion, Some((end_dt, end_az, end_el)))
        }
        _ => ("NO".to_string(), None),
    }
}

fn source_table_text(name: &str) -> String {
    let mut value: String = name.chars().take(8).collect();
    while value.chars().count() < 8 {
        value.push(' ');
    }
    value
}

fn is_sun_track(name: &str) -> bool {
    name == "Sun" || name.ends_with(" / Sun")
}

fn show_table_text_cell(ui: &mut egui::Ui, width: f32, height: f32, text: &str) {
    ui.allocate_ui_with_layout(
        egui::vec2(width, height),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.label(text);
        },
    );
}

fn show_motion_status_cell(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.allocate_ui_with_layout(
        egui::vec2(220.0, 20.0),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let mut tokens = text.split_whitespace();
            let limit = tokens.next().unwrap_or("");
            let slew = tokens.next().unwrap_or("");
            let time = tokens.next().unwrap_or("");

            show_status_token(ui, limit, 28.0);
            show_status_token(ui, slew, 28.0);
            ui.add_sized([84.0, 20.0], egui::Label::new(time));
        },
    )
    .response
}

fn skd_status_has_problem(status: &SkdRowStatus) -> bool {
    [
        &status.start_geometry,
        &status.end_geometry,
        &status.motion_1,
        &status.motion_2,
    ]
    .iter()
    .any(|value| value.contains("NO") || value.contains("Load ant"))
}

fn show_status_token(ui: &mut egui::Ui, token: &str, width: f32) {
    let color = match token {
        "OK" => egui::Color32::GREEN,
        "NO" => egui::Color32::RED,
        _ => ui.visuals().text_color(),
    };
    ui.add_sized(
        [width, 20.0],
        egui::Label::new(egui::RichText::new(token).color(color)),
    );
}

fn show_source_search_selector(
    ui: &mut egui::Ui,
    id: &'static str,
    sources: &[(Source, bool)],
    selected_index: &mut usize,
    filter_text: &mut String,
) -> bool {
    if sources.is_empty() {
        ui.label("No sources loaded");
        return false;
    }

    *selected_index = (*selected_index).min(sources.len() - 1);

    let mut changed = false;
    let search_response = ui.add_sized(
        [150.0, 22.0],
        egui::TextEdit::singleline(filter_text).hint_text("Search source"),
    );
    let query = filter_text.trim().to_lowercase();
    let matches: Vec<usize> = sources
        .iter()
        .enumerate()
        .filter_map(|(index, (source, _))| {
            (query.is_empty() || source.name.to_lowercase().contains(&query)).then_some(index)
        })
        .collect();
    if search_response.lost_focus()
        && ui.input(|input| input.key_pressed(egui::Key::Enter))
        && !matches.is_empty()
    {
        let exact_match = matches.iter().copied().find(|index| {
            sources[*index].0.name.to_lowercase() == filter_text.trim().to_lowercase()
        });
        *selected_index = exact_match.unwrap_or(matches[0]);
        changed = true;
    }

    let selected_name = sources[*selected_index].0.name.as_str();
    egui::ComboBox::from_id_salt((id, "source_results"))
        .width(170.0)
        .selected_text(selected_name)
        .show_ui(ui, |ui| {
            if matches.is_empty() {
                ui.label("No matching sources");
            } else {
                for index in matches.iter().copied() {
                    let source = &sources[index].0;
                    let label = format!(
                        "{:<16}  RA {:02}:{:02}:{:04.1}  Dec {}{:02}:{:02}:{:04.1}",
                        source.name,
                        source.ra_h,
                        source.ra_m,
                        source.ra_s,
                        source.dec_sign,
                        source.dec_d,
                        source.dec_m,
                        source.dec_s
                    );
                    if ui
                        .selectable_value(
                            selected_index,
                            index,
                            egui::RichText::new(label).monospace(),
                        )
                        .changed()
                    {
                        changed = true;
                    }
                }
            }
        });
    ui.small(matches.len().to_string())
        .on_hover_text("Matching sources");
    if !filter_text.is_empty() && ui.small_button("×").on_hover_text("Clear search").clicked() {
        filter_text.clear();
    }
    changed
}

struct UptimePlotApp {
    stations: Vec<Station>,
    selected_station: usize,
    selected_stations: Vec<bool>,
    selected_date: NaiveDate,
    station_file_path: String,
    source_file_path: String,
    antenna_file_path: String,
    input_drg_file_path: String,
    obs_code: String,
    pi_name: String,
    sources: Vec<(Source, bool)>,
    antennas: Vec<Antenna>,
    selected_antenna: usize,
    selected_antenna_2: usize,
    skd_rows: Vec<SkdRow>,
    skd_status_cache: Vec<SkdRowStatus>,
    skd_status_dirty: bool,
    selected_skd_row: Option<usize>,
    selected_skd_rows: BTreeSet<usize>,
    skd_undo_stack: Vec<Vec<SkdRow>>,
    skd_redo_stack: Vec<Vec<SkdRow>>,
    skd_slew_gap_sec: u32,
    skd_auto_slew_margin_sec: u32,
    skd_show_problems_only: bool,
    skd_scroll_to_row: Option<usize>,
    skd_show_timeline: bool,
    skd_block_repeat_count: u32,
    skd_block_gap_sec: u32,
    skd_block_clipboard: Vec<SkdRow>,
    skd_bulk_source_index: usize,
    skd_bulk_duration_sec: u32,
    skd_bulk_shift_sec: i32,
    skd_bulk_az_offset_deg: f64,
    skd_bulk_el_offset_deg: f64,
    skd_bulk_ra_offset_deg: f64,
    skd_bulk_dec_offset_deg: f64,
    new_skd_source_index: usize,
    new_skd_start_date: NaiveDate,
    new_skd_start_time: String,
    new_skd_duration_sec: u32,
    interleave_target_index: usize,
    interleave_cal_index: usize,
    interleave_start_time: String,
    interleave_target_duration_sec: u32,
    interleave_cal_duration_sec: u32,
    interleave_slew_sec: u32,
    interleave_cycles: u32,
    interleave_clear_existing: bool,
    schedule_time_shift_sec: i32,
    five_point_cal_index: usize,
    five_point_start_time: String,
    five_point_obstime_sec: u32,
    five_point_slew_sec: u32,
    five_point_offset_deg: f64,
    five_point_include_station_offsets: bool,
    five_point_clear_existing: bool,
    target_picker_filter: String,
    cal_picker_filter: String,
    five_point_picker_filter: String,
    new_skd_source_filter: String,
    skd_setup_open: bool,
    plot_data: Vec<(String, Vec<[f64; 2]>, Vec<[f64; 2]>)>,
    lst_plot_data: Vec<(String, Vec<[f64; 2]>, Vec<[f64; 2]>)>,
    polar_plot_data: Vec<(
        String,
        Vec<[f64; 2]>,
        Vec<[f64; 2]>,
        Vec<(f64, f64, String)>,
    )>,
    error_msg: Option<String>,
    show_calendar: bool,
    show_new_skd_calendar: bool,
    search_query: String,
    selected_tab: AppTab,
    uptime_plot_rect: Option<egui::Rect>,
    polar_plot_rect: Option<egui::Rect>,
    lst_plot_rect: Option<egui::Rect>,
    output_capture: Option<OutputCaptureState>,
}

impl UptimePlotApp {
    fn new(cli_args: CliArgs) -> Self {
        let app_dir = runtime_app_dir();
        let user_data_dir = uptimeplot_data_dir().unwrap_or_else(|| app_dir.clone());
        let default_source_path =
            ensure_user_data_file(&user_data_dir, "source.txt", DEFAULT_SOURCE_TXT)
                .unwrap_or_else(|| app_dir.join("source.txt"));
        let default_antenna_path =
            ensure_user_data_file(&user_data_dir, "antenna.sch", DEFAULT_ANTENNA_SCH)
                .unwrap_or_else(|| app_dir.join("antenna.sch"));
        let default_station_path =
            ensure_user_data_file(&user_data_dir, "station.txt", DEFAULT_STATION_TXT)
                .unwrap_or_else(|| app_dir.join("station.txt"));

        // Determine station_file_path
        let station_file_path = cli_args.station_path.unwrap_or(default_station_path);

        // Determine source_file_path
        let source_file_path = cli_args.source_path.unwrap_or(default_source_path);
        let input_drg_path = cli_args.drg_path;
        let _ = ensure_text_file_with_header(&station_file_path, DEFAULT_STATION_TXT);
        let _ = ensure_text_file_with_header(&source_file_path, DEFAULT_SOURCE_TXT);

        let stations: Vec<Station> = {
            let mut stations_vec = Vec::new();
            if let Ok(file) = fs::File::open(&station_file_path) {
                let reader = BufReader::new(file);
                for line in reader.lines() {
                    if let Ok(line) = line {
                        let line = line.trim();
                        if line.is_empty() || line.starts_with('#') || line.starts_with('*') {
                            continue;
                        }
                        let parts: Vec<&str> = line.split_whitespace().collect();
                        if parts.len() == 4 {
                            if let (Ok(pos_x), Ok(pos_y), Ok(pos_z)) = (
                                parts[1].parse::<f64>(),
                                parts[2].parse::<f64>(),
                                parts[3].parse::<f64>(),
                            ) {
                                stations_vec.push(Station {
                                    name: parts[0].to_string(),
                                    pos: [pos_x, pos_y, pos_z],
                                });
                            }
                        }
                    }
                }
            }
            stations_vec
        };

        let default_station_idx = if stations.is_empty() {
            0 // Default to 0 if no stations loaded, or handle error
        } else {
            stations
                .iter()
                .position(|s| s.name == "YAMAGU32")
                .unwrap_or(0)
        };

        let selected_stations: Vec<bool> = stations
            .iter()
            .enumerate()
            .map(|(i, station)| station.name == "YAMAGU32" || i == default_station_idx)
            .collect();

        let mut app = Self {
            stations,
            selected_station: default_station_idx,
            selected_stations,
            selected_date: Utc::now().date_naive(),
            station_file_path: station_file_path.to_str().unwrap_or_default().to_string(),
            source_file_path: source_file_path.to_str().unwrap_or_default().to_string(),
            antenna_file_path: default_antenna_path
                .to_str()
                .unwrap_or_default()
                .to_string(),
            input_drg_file_path: String::new(),
            obs_code: String::new(),
            pi_name: "hogehoge".to_string(),
            sources: Vec::new(),
            antennas: Vec::new(),
            selected_antenna: 0,
            selected_antenna_2: 0,
            skd_rows: Vec::new(),
            skd_status_cache: Vec::new(),
            skd_status_dirty: true,
            selected_skd_row: None,
            selected_skd_rows: BTreeSet::new(),
            skd_undo_stack: Vec::new(),
            skd_redo_stack: Vec::new(),
            skd_slew_gap_sec: 60,
            skd_auto_slew_margin_sec: 0,
            skd_show_problems_only: false,
            skd_scroll_to_row: None,
            skd_show_timeline: false,
            skd_block_repeat_count: 1,
            skd_block_gap_sec: 60,
            skd_block_clipboard: Vec::new(),
            skd_bulk_source_index: 0,
            skd_bulk_duration_sec: 240,
            skd_bulk_shift_sec: 0,
            skd_bulk_az_offset_deg: 0.0,
            skd_bulk_el_offset_deg: 0.0,
            skd_bulk_ra_offset_deg: 0.0,
            skd_bulk_dec_offset_deg: 0.0,
            new_skd_source_index: 0,
            new_skd_start_date: Utc::now().date_naive(),
            new_skd_start_time: "00:00:00".to_string(),
            new_skd_duration_sec: 240,
            interleave_target_index: 0,
            interleave_cal_index: 0,
            interleave_start_time: "00:00:00".to_string(),
            interleave_target_duration_sec: 1440,
            interleave_cal_duration_sec: 240,
            interleave_slew_sec: 60,
            interleave_cycles: 10,
            interleave_clear_existing: false,
            schedule_time_shift_sec: 0,
            five_point_cal_index: 0,
            five_point_start_time: "00:00:00".to_string(),
            five_point_obstime_sec: 60,
            five_point_slew_sec: 30,
            five_point_offset_deg: DEFAULT_FIVE_POINT_OFFSET_ARCMIN,
            five_point_include_station_offsets: true,
            five_point_clear_existing: false,
            target_picker_filter: String::new(),
            cal_picker_filter: String::new(),
            five_point_picker_filter: String::new(),
            new_skd_source_filter: String::new(),
            skd_setup_open: true,
            plot_data: Vec::new(),
            lst_plot_data: Vec::new(),
            polar_plot_data: Vec::new(),
            error_msg: None,
            show_calendar: false,
            show_new_skd_calendar: false,
            search_query: String::new(),
            selected_tab: if input_drg_path.is_some() {
                AppTab::SkdTable
            } else {
                AppTab::UptimePlotters
            },
            uptime_plot_rect: None,
            polar_plot_rect: None,
            lst_plot_rect: None,
            output_capture: None,
        };
        let _ = app.load_sources();
        let _ = app.load_antennas();
        if let Some(path) = input_drg_path {
            app.input_drg_file_path = path.to_string_lossy().to_string();
            if let Err(error) = app.load_drg_file() {
                app.error_msg = Some(error);
            }
        }

        app
    }
}

impl eframe::App for UptimePlotApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.show_calendar_window(&ctx);
        self.show_new_skd_calendar_window(&ctx);

        if let Some(image) = ctx.input(|i| {
            i.events.iter().find_map(|e| {
                if let egui::Event::Screenshot { image, .. } = e {
                    Some(image.clone())
                } else {
                    None
                }
            })
        }) {
            self.handle_output_screenshot(&image, ctx.pixels_per_point());
        }

        egui::Panel::top("top_panel").show_inside(ui, |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.selected_tab, AppTab::Parameters, "Parameters");
                ui.selectable_value(
                    &mut self.selected_tab,
                    AppTab::UptimePlotters,
                    "Uptime Plotters",
                );
                ui.selectable_value(&mut self.selected_tab, AppTab::PolarPlot, "Polar Plot");
                ui.selectable_value(&mut self.selected_tab, AppTab::LstPlot, "LST Plot");
                ui.selectable_value(&mut self.selected_tab, AppTab::SkdTable, "SKD Table");
            });
        });

        egui::CentralPanel::default().show_inside(ui, |ui| match self.selected_tab {
            AppTab::UptimePlotters => self.ui_uptime_plotters_tab(ui),
            AppTab::Parameters => self.ui_parameters_tab(ui),
            AppTab::PolarPlot => self.ui_polar_plot_tab(ui),
            AppTab::LstPlot => self.ui_lst_plot_tab(ui),
            AppTab::SkdTable => self.ui_skd_table_tab(ui),
        });

        self.drive_output_capture(&ctx);
    }
}

impl UptimePlotApp {
    fn load_sources(&mut self) -> Result<(), String> {
        ensure_text_file_with_header(Path::new(&self.source_file_path), DEFAULT_SOURCE_TXT)?;
        let source_content = fs::read_to_string(&self.source_file_path)
            .map_err(|e| format!("Failed to read source file: {}", e))?;

        let mut sources = Vec::new();
        for line in source_content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with('*') {
                continue;
            }
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() < 7 {
                continue;
            }

            let source = parse_source_tokens(&parts, 0, 1, line)?;
            sources.push((source, false));
        }
        self.sources = sources;
        self.clear_plot_data();
        self.mark_skd_status_dirty();
        Ok(())
    }

    fn load_antennas(&mut self) -> Result<(), String> {
        let content = fs::read_to_string(&self.antenna_file_path)
            .map_err(|e| format!("Failed to read antenna.sch: {}", e))?;

        let mut antennas = Vec::new();
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('*') || line.starts_with("LTRCODE") {
                continue;
            }
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() < 15 {
                continue;
            }
            let parsed = (
                parts[2].parse::<f64>(),
                parts[3].parse::<f64>(),
                parts[4].parse::<f64>(),
                parts[7].parse::<f64>(),
                parts[9].parse::<f64>(),
                parts[10].parse::<f64>(),
                parts[11].parse::<f64>(),
                parts[13].parse::<f64>(),
                parts[14].parse::<f64>(),
            );
            if let (
                Ok(x),
                Ok(y),
                Ok(z),
                Ok(az_rate),
                Ok(az_min),
                Ok(az_max),
                Ok(el_rate),
                Ok(el_min),
                Ok(el_max),
            ) = parsed
            {
                antennas.push(Antenna {
                    code: parts[0].to_string(),
                    name: parts[1].to_string(),
                    pos: [x, y, z],
                    az_rate_deg_per_min: az_rate,
                    az_min_deg: az_min,
                    az_max_deg: az_max,
                    el_rate_deg_per_min: el_rate,
                    el_min_deg: el_min,
                    el_max_deg: el_max,
                });
            }
        }

        if antennas.is_empty() {
            return Err("No antennas loaded from antenna.sch.".to_string());
        }
        self.antennas = antennas;
        self.selected_antenna = self
            .antennas
            .iter()
            .position(|antenna| antenna.name == "YAMAGU32")
            .unwrap_or(0);
        self.selected_antenna_2 = self
            .antennas
            .iter()
            .position(|antenna| antenna.name == "YAMAGU34")
            .unwrap_or_else(|| {
                if self.antennas.len() > 1 {
                    (self.selected_antenna + 1) % self.antennas.len()
                } else {
                    self.selected_antenna
                }
            });
        self.mark_skd_status_dirty();
        Ok(())
    }

    fn clear_plot_data(&mut self) {
        self.plot_data.clear();
        self.lst_plot_data.clear();
        self.polar_plot_data.clear();
    }

    fn selected_station_indices(&self) -> Vec<usize> {
        self.selected_stations
            .iter()
            .enumerate()
            .filter_map(|(i, selected)| {
                if *selected && i < self.stations.len() {
                    Some(i)
                } else {
                    None
                }
            })
            .collect()
    }

    fn load_stations(&mut self) -> Result<(), String> {
        let old_selected_names: Vec<String> = self
            .stations
            .iter()
            .enumerate()
            .filter_map(|(i, station)| {
                if self.selected_stations.get(i).copied().unwrap_or(false) {
                    Some(station.name.clone())
                } else {
                    None
                }
            })
            .collect();

        ensure_text_file_with_header(Path::new(&self.station_file_path), DEFAULT_STATION_TXT)?;
        let station_content = fs::read_to_string(&self.station_file_path)
            .map_err(|e| format!("Failed to read station file: {}", e))?;

        let mut stations_vec = Vec::new();
        for line in station_content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with('*') {
                continue;
            }
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() == 4 {
                if let (Ok(pos_x), Ok(pos_y), Ok(pos_z)) = (
                    parts[1].parse::<f64>(),
                    parts[2].parse::<f64>(),
                    parts[3].parse::<f64>(),
                ) {
                    stations_vec.push(Station {
                        name: parts[0].to_string(),
                        pos: [pos_x, pos_y, pos_z],
                    });
                } else {
                    return Err(format!("Invalid number format in station file: {}", line));
                }
            } else {
                return Err(format!("Invalid line format in station file: {}", line));
            }
        }
        self.stations = stations_vec;

        // Reset selected_station if the current one is no longer valid.
        if self.selected_station >= self.stations.len() {
            self.selected_station = 0;
        }

        // Preserve previous checkbox selections by station name when reloading.
        self.selected_stations = self
            .stations
            .iter()
            .enumerate()
            .map(|(i, station)| {
                old_selected_names.iter().any(|name| name == &station.name)
                    || station.name == "YAMAGU32"
                    || i == self.selected_station
            })
            .collect();

        if !self.selected_stations.iter().any(|selected| *selected)
            && !self.selected_stations.is_empty()
        {
            self.selected_stations[0] = true;
            self.selected_station = 0;
        }

        self.clear_plot_data();
        Ok(())
    }

    fn calculate_plots(&mut self) {
        // println!("DEBUG: calculate_plots called.");
        // println!("DEBUG: self.stations.len() = {}", self.stations.len());
        // println!("DEBUG: self.selected_station = {}", self.selected_station);

        if self.stations.is_empty() {
            self.error_msg = Some("No stations loaded. Please check station.txt".to_string());
            return;
        }

        let station_indices = self.selected_station_indices();
        if station_indices.is_empty() {
            self.error_msg = Some("No stations selected.".to_string());
            return;
        }

        let mut new_plot_data = Vec::new();
        let mut new_lst_plot_data = Vec::new();

        for station_idx in station_indices {
            let station = &self.stations[station_idx];
            let ant_pos = station.pos;

            for (source, selected) in &self.sources {
                if !*selected {
                    continue;
                }

                let mut az_points = Vec::new();
                let mut el_points = Vec::new();
                let mut lst_az_points = Vec::new();
                let mut lst_el_points = Vec::new();
                let mut prev_lst: Option<f64> = None;

                for i in (0..=(24 * 60)).step_by(3) {
                    // 3 minute intervals
                    let hour_float = (i as f64) / 60.0;
                    let h = (i / 60) as u32;
                    let m = (i % 60) as u32;

                    if let Some(time) = self.selected_date.and_hms_opt(h, m, 0) {
                        let datetime_utc = Utc.from_utc_datetime(&time);
                        let (az, el, _) = utils::radec2azalt(
                            ant_pos,
                            datetime_utc,
                            source.ra_rad,
                            source.dec_rad,
                        );

                        az_points.push([hour_float, az]);
                        if el >= PLOT_MIN_ELEVATION_DEG {
                            el_points.push([hour_float, el]);
                        } else {
                            el_points.push([hour_float, f64::NAN]);
                        }

                        let lst_hour = utils::utc_to_lst_hours(ant_pos, datetime_utc);
                        if let Some(prev) = prev_lst {
                            if lst_hour + 12.0 < prev {
                                lst_az_points.push([f64::NAN, f64::NAN]);
                                lst_el_points.push([f64::NAN, f64::NAN]);
                            }
                        }

                        lst_az_points.push([lst_hour, az]);
                        if el >= PLOT_MIN_ELEVATION_DEG {
                            lst_el_points.push([lst_hour, el]);
                        } else {
                            lst_el_points.push([lst_hour, f64::NAN]);
                        }
                        prev_lst = Some(lst_hour);
                    }
                }

                let label = format!("{} / {}", station.name, source.name);
                new_plot_data.push((label.clone(), az_points, el_points));
                new_lst_plot_data.push((label, lst_az_points, lst_el_points));
            }

            let mut sun_az_points = Vec::new();
            let mut sun_el_points = Vec::new();
            let mut sun_lst_az_points = Vec::new();
            let mut sun_lst_el_points = Vec::new();
            let mut previous_sun_lst: Option<f64> = None;

            for minute_of_day in (0..=(24 * 60)).step_by(3) {
                let hour_float = minute_of_day as f64 / 60.0;
                let hour = (minute_of_day / 60) as u32;
                let minute = (minute_of_day % 60) as u32;

                if let Some(time) = self.selected_date.and_hms_opt(hour, minute, 0) {
                    let datetime_utc = Utc.from_utc_datetime(&time);
                    let (sun_ra, sun_dec) = utils::sun_radec(datetime_utc);
                    let (az, el, _) = utils::radec2azalt(ant_pos, datetime_utc, sun_ra, sun_dec);

                    sun_az_points.push([hour_float, az]);
                    if el >= PLOT_MIN_ELEVATION_DEG {
                        sun_el_points.push([hour_float, el]);
                    } else {
                        sun_el_points.push([hour_float, f64::NAN]);
                    }

                    let lst_hour = utils::utc_to_lst_hours(ant_pos, datetime_utc);
                    if let Some(previous_lst) = previous_sun_lst {
                        if lst_hour + 12.0 < previous_lst {
                            sun_lst_az_points.push([f64::NAN, f64::NAN]);
                            sun_lst_el_points.push([f64::NAN, f64::NAN]);
                        }
                    }
                    sun_lst_az_points.push([lst_hour, az]);
                    if el >= PLOT_MIN_ELEVATION_DEG {
                        sun_lst_el_points.push([lst_hour, el]);
                    } else {
                        sun_lst_el_points.push([lst_hour, f64::NAN]);
                    }
                    previous_sun_lst = Some(lst_hour);
                }
            }

            let sun_label = format!("{} / Sun", station.name);
            new_plot_data.push((sun_label.clone(), sun_az_points, sun_el_points));
            new_lst_plot_data.push((sun_label, sun_lst_az_points, sun_lst_el_points));
        }

        self.plot_data = new_plot_data;
        self.lst_plot_data = new_lst_plot_data;
        self.polar_plot_data = self.build_polar_plot_data();
        self.error_msg = None;
    }

    fn station_position(&self) -> Option<[f64; 3]> {
        self.selected_station_indices()
            .first()
            .and_then(|&i| self.stations.get(i))
            .or_else(|| self.stations.get(self.selected_station))
            .map(|station| station.pos)
    }

    fn lst_from_ut_hour(&self, station_pos: [f64; 3], ut_hour: f64) -> Option<f64> {
        let datetime = utc_datetime_from_hour(self.selected_date, ut_hour)?;
        Some(utils::utc_to_lst_hours(station_pos, datetime))
    }

    fn build_polar_plot_data(
        &self,
    ) -> Vec<(
        String,
        Vec<[f64; 2]>,
        Vec<[f64; 2]>,
        Vec<(f64, f64, String)>,
    )> {
        let mut polar_plot_data = Vec::new();

        for (name, az_points, el_points) in &self.plot_data {
            let mut polar_points = Vec::new();
            let mut hour_marker_points = Vec::new();
            let mut hour_labels = Vec::new();

            for i in 0..az_points.len() {
                let hour = az_points[i][0];
                let az = az_points[i][1];
                let el = el_points[i][1];

                if !el.is_nan() && el >= PLOT_MIN_ELEVATION_DEG {
                    let angle_rad = (90.0f64 - az).to_radians();
                    let radius = (90.0 - el) / 90.0;
                    let x = radius * angle_rad.cos();
                    let y = radius * angle_rad.sin();
                    polar_points.push([x, y]);

                    if (hour - hour.round()).abs() < 1e-6 {
                        hour_marker_points.push([x, y]);
                        let label_hour = hour.round() as i32;
                        let label_offset = 0.04;
                        hour_labels.push((
                            x + angle_rad.cos() * label_offset,
                            y + angle_rad.sin() * label_offset,
                            format!("{:02}h", label_hour),
                        ));
                    }
                }
            }

            polar_plot_data.push((name.clone(), polar_points, hour_marker_points, hour_labels));
        }

        polar_plot_data
    }

    fn output_target_tab(target: OutputTarget) -> AppTab {
        match target {
            OutputTarget::UtAzel => AppTab::UptimePlotters,
            OutputTarget::Polar => AppTab::PolarPlot,
            OutputTarget::Lst => AppTab::LstPlot,
        }
    }

    fn output_target_path(target: OutputTarget) -> PathBuf {
        let mut path = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        match target {
            OutputTarget::UtAzel => path.push("upt_azel.png"),
            OutputTarget::Polar => path.push("upt_polar.png"),
            OutputTarget::Lst => path.push("upt_lst.png"),
        }
        path
    }

    fn output_target_rect(&self, target: OutputTarget) -> Option<egui::Rect> {
        match target {
            OutputTarget::UtAzel => self.uptime_plot_rect,
            OutputTarget::Polar => self.polar_plot_rect,
            OutputTarget::Lst => self.lst_plot_rect,
        }
    }

    fn start_output_capture(&mut self, ctx: &egui::Context) -> Result<(), String> {
        if self.plot_data.is_empty() {
            return Err("No plot data to output. Please run Plot Selected first.".to_string());
        }
        if self.station_position().is_none() {
            return Err("No station selected.".to_string());
        }
        if self.output_capture.is_some() {
            return Err("Output is already running.".to_string());
        }

        let previous_tab = self.selected_tab;
        self.output_capture = Some(OutputCaptureState {
            targets: vec![OutputTarget::UtAzel, OutputTarget::Polar, OutputTarget::Lst],
            index: 0,
            previous_tab,
            screenshot_requested: false,
        });
        self.uptime_plot_rect = None;
        self.polar_plot_rect = None;
        self.lst_plot_rect = None;
        self.selected_tab = AppTab::UptimePlotters;
        ctx.request_repaint();
        Ok(())
    }

    fn drive_output_capture(&mut self, ctx: &egui::Context) {
        let (target, screenshot_requested) = match self.output_capture.as_ref() {
            Some(state) => match state.targets.get(state.index).copied() {
                Some(target) => (target, state.screenshot_requested),
                None => return,
            },
            None => return,
        };

        if screenshot_requested {
            return;
        }

        let target_tab = Self::output_target_tab(target);
        if self.selected_tab != target_tab {
            self.selected_tab = target_tab;
            ctx.request_repaint();
            return;
        }

        if self.output_target_rect(target).is_none() {
            ctx.request_repaint();
            return;
        }

        ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
        if let Some(state) = self.output_capture.as_mut() {
            state.screenshot_requested = true;
        }
        ctx.request_repaint();
    }

    fn handle_output_screenshot(&mut self, image: &egui::ColorImage, pixels_per_point: f32) {
        let (target, screenshot_requested, previous_tab) = match self.output_capture.as_ref() {
            Some(state) => match state.targets.get(state.index).copied() {
                Some(target) => (target, state.screenshot_requested, state.previous_tab),
                None => return,
            },
            None => return,
        };

        if !screenshot_requested {
            return;
        }

        let rect = match self.output_target_rect(target) {
            Some(rect) => rect,
            None => {
                self.error_msg = Some("Failed to capture plot area.".to_string());
                self.selected_tab = previous_tab;
                self.output_capture = None;
                return;
            }
        };

        let output_path = Self::output_target_path(target);
        if let Err(e) = save_plot_region_png(image, rect, pixels_per_point, &output_path) {
            self.error_msg = Some(e);
            self.selected_tab = previous_tab;
            self.output_capture = None;
            return;
        }

        let mut next_target: Option<OutputTarget> = None;
        let mut done = false;
        if let Some(state) = self.output_capture.as_mut() {
            state.index += 1;
            state.screenshot_requested = false;
            if state.index >= state.targets.len() {
                done = true;
            } else {
                next_target = state.targets.get(state.index).copied();
            }
        }

        if done {
            self.selected_tab = previous_tab;
            self.output_capture = None;
            self.error_msg =
                Some("Output complete: upt_azel.png, upt_polar.png, upt_lst.png".to_string());
        } else if let Some(next) = next_target {
            match next {
                OutputTarget::UtAzel => self.uptime_plot_rect = None,
                OutputTarget::Polar => self.polar_plot_rect = None,
                OutputTarget::Lst => self.lst_plot_rect = None,
            }
            self.selected_tab = Self::output_target_tab(next);
        }
    }

    fn show_calendar_window(&mut self, ctx: &egui::Context) {
        if self.show_calendar {
            let previous_date = self.selected_date;
            let mut open = true;
            egui::Window::new("Select Date")
                .open(&mut open)
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    if calendar_ui(ui, &mut self.selected_date) {
                        self.show_calendar = false;
                    }
                });
            if self.selected_tab == AppTab::SkdTable && self.selected_date != previous_date {
                self.shift_skd_rows_by_days((self.selected_date - previous_date).num_days());
                self.new_skd_start_date = self.selected_date;
            }
            if !open {
                self.show_calendar = false;
            }
        }
    }

    fn show_new_skd_calendar_window(&mut self, ctx: &egui::Context) {
        if self.show_new_skd_calendar {
            let mut open = true;
            egui::Window::new("Select Date for New Row")
                .open(&mut open)
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    if calendar_ui(ui, &mut self.new_skd_start_date) {
                        self.show_new_skd_calendar = false;
                    }
                });
            if !open {
                self.show_new_skd_calendar = false;
            }
        }
    }

    #[allow(dead_code)]
    fn save_plot_data_to_csv(&self) -> Result<(), String> {
        if self.plot_data.is_empty() {
            return Err("No plot data to save.".to_string());
        }

        let mut csv_content = String::new();
        let mut header = "Time".to_string();
        let mut time_points: Vec<f64> = Vec::new();

        // Collect all unique time points and build header
        for (i, (name, az_points, _)) in self.plot_data.iter().enumerate() {
            header.push_str(&format!(",{},{}", name, name)); // Add source name twice for AZ and EL
            if i == 0 {
                // Assuming time points are common for all sources
                time_points = az_points.iter().map(|p| p[0]).collect();
            }
        }
        csv_content.push_str(&header);
        csv_content.push_str("\n");

        // Populate data rows
        for &time in &time_points {
            let mut row = format!("{:.2}", time); // Format time to 2 decimal places
            for (_, az_points, el_points) in &self.plot_data {
                // Find the corresponding az and el for this time
                let az_val = az_points
                    .iter()
                    .find(|p| (p[0] - time).abs() < 1e-6)
                    .map_or("".to_string(), |p| format!("{:.1}", p[1]));
                let el_val = el_points
                    .iter()
                    .find(|p| (p[0] - time).abs() < 1e-6)
                    .map_or("".to_string(), |p| format!("{:.1}", p[1]));
                row.push_str(&format!(",{},{}", az_val, el_val));
            }
            csv_content.push_str(&row);
            csv_content.push_str("\n");
        }

        let mut path = runtime_app_dir();
        path.push("plot_data.csv");
        fs::write(&path, csv_content).map_err(|e| format!("Failed to save CSV file: {}", e))?;
        Ok(())
    }

    fn load_drg_file(&mut self) -> Result<(), String> {
        let path = output_drg_path(&self.input_drg_file_path)?;
        let content = fs::read_to_string(&path)
            .map_err(|e| format!("Failed to read DRG file {}: {}", path.display(), e))?;

        if let Some(obs_code) = parse_exper_code(&content) {
            self.obs_code = obs_code;
        }
        if let Some(pi_name) = parse_pi_name(&content) {
            self.pi_name = pi_name;
        }

        let mut sources = Vec::new();
        for line in section_lines(&content, "$SOURCES")? {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('*') {
                continue;
            }
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            if parts.len() >= 9 {
                sources.push((parse_source_tokens(&parts, 0, 2, trimmed)?, false));
            }
        }
        if !sources.is_empty() {
            let mut added_source = false;
            for (drg_source, _) in sources {
                if !self
                    .sources
                    .iter()
                    .any(|(source, _)| source.name == drg_source.name)
                {
                    self.sources.push((drg_source, false));
                    added_source = true;
                }
            }
            if added_source {
                self.clear_plot_data();
                self.mark_skd_status_dirty();
            }
            self.interleave_target_index = self.interleave_target_index.min(self.sources.len() - 1);
            self.interleave_cal_index = self.interleave_cal_index.min(self.sources.len() - 1);
            self.new_skd_source_index = self.new_skd_source_index.min(self.sources.len() - 1);
            self.five_point_cal_index = self.five_point_cal_index.min(self.sources.len() - 1);
        }

        let mut rows = Vec::new();
        for line in section_lines(&content, "$SKED")? {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('*') {
                continue;
            }
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            if let Some(start_idx) = parts
                .iter()
                .position(|part| part.len() == 11 && part.chars().all(|c| c.is_ascii_digit()))
            {
                let (start_date, start_time) = parse_drg_timestamp(parts[start_idx])?;
                let duration_sec = parts
                    .get(start_idx + 1)
                    .and_then(|part| part.parse::<u32>().ok())
                    .ok_or_else(|| format!("Invalid SKED duration: {}", trimmed))?;
                let az_offset_deg = parts
                    .get(start_idx + 2)
                    .and_then(|part| part.parse::<f64>().ok())
                    .unwrap_or(0.0);
                let el_offset_deg = parts
                    .get(start_idx + 3)
                    .and_then(|part| part.parse::<f64>().ok())
                    .unwrap_or(0.0);
                let ra_offset_deg = parts
                    .get(start_idx + 4)
                    .and_then(|part| part.parse::<f64>().ok())
                    .unwrap_or(0.0);
                let dec_offset_deg = parts
                    .get(start_idx + 5)
                    .and_then(|part| part.parse::<f64>().ok())
                    .unwrap_or(0.0);
                rows.push(SkdRow {
                    source_name: parts[0].to_string(),
                    start_date,
                    start_time,
                    duration_sec,
                    az_offset_deg,
                    el_offset_deg,
                    ra_offset_deg,
                    dec_offset_deg,
                    include_station_offsets: false,
                });
            }
        }
        self.skd_rows = rows;
        self.sort_skd_rows_by_start_time();
        // Loading a DRG always returns generators to the non-destructive mode.
        // A replacement must be explicitly requested after the file is loaded.
        self.interleave_clear_existing = false;
        self.five_point_clear_existing = false;
        self.skd_undo_stack.clear();
        self.skd_redo_stack.clear();
        self.skd_block_clipboard.clear();
        if self.skd_rows.is_empty() {
            self.clear_skd_selection();
        } else {
            self.select_only_skd_row(0);
        }
        if let Some(first_row) = self.skd_rows.first() {
            self.selected_date = first_row.start_date;
        }
        Ok(())
    }

    fn output_directory_from_input_drg(&self) -> Option<PathBuf> {
        let input_path = output_drg_path(&self.input_drg_file_path).ok()?;
        input_path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .map(Path::to_path_buf)
    }

    fn path_in_output_directory(&self, path: PathBuf) -> PathBuf {
        let Some(output_dir) = self.output_directory_from_input_drg() else {
            return path;
        };
        let Some(file_name) = path.file_name() else {
            return path;
        };
        output_dir.join(file_name)
    }

    fn obs_code_output_path(&self, obs_code: &str) -> Result<PathBuf, String> {
        obs_code_output_path(obs_code).map(|path| self.path_in_output_directory(path))
    }

    fn obs_code_station_skd_output_path(
        &self,
        obs_code: &str,
        station_suffix: &str,
    ) -> Result<PathBuf, String> {
        obs_code_station_skd_output_path(obs_code, station_suffix)
            .map(|path| self.path_in_output_directory(path))
    }

    fn write_skd_to_drg(&self) -> Result<Vec<PathBuf>, String> {
        if self.skd_rows.is_empty() {
            return Err("No SKED rows to write.".to_string());
        }
        let obs_code = self.obs_code.trim();
        if obs_code.is_empty() || obs_code.split_whitespace().count() != 1 {
            return Err("Obscode must be one non-empty word.".to_string());
        }
        let drg_output_path = self.obs_code_output_path(obs_code)?;
        let skd32_output_path = self.obs_code_station_skd_output_path(obs_code, "32")?;
        let skd34_output_path = self.obs_code_station_skd_output_path(obs_code, "34")?;

        let mut source_names: Vec<String> = Vec::new();
        for row in &self.skd_rows {
            parse_time_string(&row.start_time)?;
            if self.find_source(&row.source_name).is_none() {
                return Err(format!(
                    "Source '{}' is not loaded in Source Settings.",
                    row.source_name
                ));
            }
            if !source_names.iter().any(|name| name == &row.source_name) {
                source_names.push(row.source_name.clone());
            }
        }

        let mut sources_section = String::from("$SOURCES\n");
        for source_name in &source_names {
            let source = self.find_source(source_name).unwrap();
            sources_section.push_str(&format_source_drg_line(source));
            sources_section.push('\n');
        }
        sources_section.push_str("*\n");

        let mut drg_sked_section = String::from("$SKED\n");
        drg_sked_section
            .push_str("*SOURCES CAL FR          START     DUR       IDLE       STATIONS  TAPE\n");
        for row in &self.skd_rows {
            let start = format_drg_timestamp(row.start_date, &row.start_time)?;
            drg_sked_section.push_str(&format!(
                "{:<8}  10 S2  PREOB {} {:>4}  MIDOB 0 POSTOB K-L- 1F00000 1F00000\n",
                row.source_name, start, row.duration_sec
            ));
        }
        drg_sked_section.push_str("*\n");

        let drg_content =
            build_new_drg_content(obs_code, &self.pi_name, &sources_section, &drg_sked_section);
        let skd32_content =
            build_simple_skd_content(obs_code, &sources_section, &self.skd_rows, Some("32"))?;
        let skd34_content =
            build_simple_skd_content(obs_code, &sources_section, &self.skd_rows, Some("34"))?;
        fs::write(&drg_output_path, drg_content)
            .map_err(|e| format!("Failed to write output DRG file: {}", e))?;
        fs::write(&skd32_output_path, skd32_content)
            .map_err(|e| format!("Failed to write output SKD32 file: {}", e))?;
        fs::write(&skd34_output_path, skd34_content)
            .map_err(|e| format!("Failed to write output SKD34 file: {}", e))?;
        Ok(vec![drg_output_path, skd32_output_path, skd34_output_path])
    }

    fn generate_five_point_skd_rows(&mut self) -> Result<(), String> {
        if self.sources.is_empty() {
            return Err("Load source.txt first.".to_string());
        }
        self.five_point_cal_index = self.five_point_cal_index.min(self.sources.len() - 1);
        parse_time_string(&self.five_point_start_time)?;
        if self.five_point_obstime_sec == 0 {
            return Err("Obs Time must be at least 1 second.".to_string());
        }

        let cal_name = self.sources[self.five_point_cal_index].0.name.clone();
        let mut generated = Vec::new();
        let mut offset_sec = 0_i64;
        let offset_pattern = five_point_offset_pattern(self.five_point_offset_deg);
        for &(az_offset_deg, el_offset_deg) in &offset_pattern {
            let (start_date, start_time) =
                offset_schedule_time(self.selected_date, &self.five_point_start_time, offset_sec)?;
            generated.push(SkdRow {
                source_name: cal_name.clone(),
                start_date,
                start_time,
                duration_sec: self.five_point_obstime_sec,
                az_offset_deg,
                el_offset_deg,
                ra_offset_deg: 0.0,
                dec_offset_deg: 0.0,
                include_station_offsets: self.five_point_include_station_offsets,
            });
            offset_sec += self.five_point_obstime_sec as i64 + self.five_point_slew_sec as i64;
        }

        self.record_skd_undo();
        if self.five_point_clear_existing {
            self.skd_rows = generated;
        } else {
            self.skd_rows.extend(generated);
        }
        self.sort_skd_rows_by_start_time();
        Ok(())
    }

    fn generate_interleaved_skd_rows(&mut self) -> Result<(), String> {
        if self.sources.is_empty() {
            return Err("Load source.txt first.".to_string());
        }
        self.interleave_target_index = self.interleave_target_index.min(self.sources.len() - 1);
        self.interleave_cal_index = self.interleave_cal_index.min(self.sources.len() - 1);
        parse_time_string(&self.interleave_start_time)?;
        if self.interleave_cycles == 0 {
            return Err("Cycle count must be at least 1.".to_string());
        }
        if self.interleave_target_duration_sec == 0 || self.interleave_cal_duration_sec == 0 {
            return Err("Durations must be at least 1 second.".to_string());
        }

        let target_name = self.sources[self.interleave_target_index].0.name.clone();
        let cal_name = self.sources[self.interleave_cal_index].0.name.clone();
        let mut generated = Vec::new();
        let mut offset_sec = 0_i64;

        for cycle_idx in 0..self.interleave_cycles {
            let (cal_date, cal_time) =
                offset_schedule_time(self.selected_date, &self.interleave_start_time, offset_sec)?;
            generated.push(SkdRow {
                source_name: cal_name.clone(),
                start_date: cal_date,
                start_time: cal_time,
                duration_sec: self.interleave_cal_duration_sec,
                az_offset_deg: 0.0,
                el_offset_deg: 0.0,
                ra_offset_deg: 0.0,
                dec_offset_deg: 0.0,
                include_station_offsets: false,
            });
            offset_sec += self.interleave_cal_duration_sec as i64 + self.interleave_slew_sec as i64;

            let (target_date, target_time) =
                offset_schedule_time(self.selected_date, &self.interleave_start_time, offset_sec)?;
            generated.push(SkdRow {
                source_name: target_name.clone(),
                start_date: target_date,
                start_time: target_time,
                duration_sec: self.interleave_target_duration_sec,
                az_offset_deg: 0.0,
                el_offset_deg: 0.0,
                ra_offset_deg: 0.0,
                dec_offset_deg: 0.0,
                include_station_offsets: false,
            });
            offset_sec += self.interleave_target_duration_sec as i64;
            if cycle_idx + 1 < self.interleave_cycles {
                offset_sec += self.interleave_slew_sec as i64;
            }
        }

        self.record_skd_undo();
        if self.interleave_clear_existing {
            self.skd_rows = generated;
        } else {
            self.skd_rows.extend(generated);
        }
        self.sort_skd_rows_by_start_time();
        Ok(())
    }

    fn shift_skd_rows_by_days(&mut self, days: i64) {
        if days == 0 {
            return;
        }
        self.record_skd_undo();
        for row in &mut self.skd_rows {
            row.start_date = row.start_date + Duration::days(days);
        }
        self.sort_skd_rows_by_start_time();
    }

    fn apply_schedule_time_shift(&mut self) -> Result<(), String> {
        if self.skd_rows.is_empty() {
            return Err("No SKED rows to shift.".to_string());
        }
        let shift_sec = self.schedule_time_shift_sec as i64;
        let mut updated = self.skd_rows.clone();
        for row in &mut updated {
            let shifted =
                schedule_datetime(row.start_date, &row.start_time)? + Duration::seconds(shift_sec);
            row.start_date = shifted.date();
            row.start_time = format!(
                "{:02}:{:02}:{:02}",
                shifted.time().hour(),
                shifted.time().minute(),
                shifted.time().second()
            );
        }
        self.record_skd_undo();
        self.skd_rows = updated;
        self.sort_skd_rows_by_start_time();
        Ok(())
    }

    fn sort_skd_rows_by_start_time(&mut self) {
        self.skd_rows.sort_by(|a, b| {
            let a_dt = schedule_datetime(a.start_date, &a.start_time).ok();
            let b_dt = schedule_datetime(b.start_date, &b.start_time).ok();
            a_dt.cmp(&b_dt)
        });
        self.clear_skd_selection();
        self.mark_skd_status_dirty();
    }

    fn mark_skd_status_dirty(&mut self) {
        self.skd_status_dirty = true;
    }

    fn rebuild_skd_status_cache(&mut self) {
        if !self.skd_status_dirty && self.skd_status_cache.len() == self.skd_rows.len() {
            return;
        }

        let mut selected_antenna_indices = Vec::new();
        for index in [self.selected_antenna, self.selected_antenna_2] {
            if index < self.antennas.len() && !selected_antenna_indices.contains(&index) {
                selected_antenna_indices.push(index);
            }
        }
        let selected_antennas: Vec<Antenna> = selected_antenna_indices
            .iter()
            .filter_map(|&index| self.antennas.get(index).cloned())
            .collect();
        let ant_pos = selected_antennas
            .first()
            .map(|antenna| antenna.pos)
            .or_else(|| self.station_position());
        let source_map: HashMap<&str, &Source> = self
            .sources
            .iter()
            .map(|(source, _)| (source.name.as_str(), source))
            .collect();
        let mut cache = Vec::with_capacity(self.skd_rows.len());
        let mut prev_ends: Vec<Option<ScanEnd>> = vec![None; selected_antennas.len()];

        for row in &self.skd_rows {
            let Some(source) = source_map.get(row.source_name.as_str()).copied() else {
                cache.push(SkdRowStatus {
                    start_geometry: "NO".to_string(),
                    end_geometry: "NO".to_string(),
                    motion_1: "NO".to_string(),
                    motion_2: "NO".to_string(),
                });
                prev_ends.fill(None);
                continue;
            };

            let (start_geometry, end_geometry) = match ant_pos {
                Some(pos) => match (
                    scan_az_el_for(row, source, pos, false),
                    scan_az_el_for(row, source, pos, true),
                ) {
                    (Some((_, start_az, start_el)), Some((_, end_az, end_el))) => (
                        format!("{:5.1}/{:5.1}", start_az, start_el),
                        format!("{:5.1}/{:5.1}", end_az, end_el),
                    ),
                    _ => ("NO".to_string(), "NO".to_string()),
                },
                None => ("No antenna".to_string(), "No antenna".to_string()),
            };

            let mut motion_values = vec![String::new(), String::new()];
            if selected_antennas.is_empty() {
                motion_values[0] = "Load ant".to_string();
            } else {
                for (ant_idx, antenna) in selected_antennas.iter().take(2).enumerate() {
                    let (antenna_motion, current_end) =
                        antenna_motion_status(row, source, antenna, prev_ends[ant_idx]);
                    prev_ends[ant_idx] = current_end;
                    motion_values[ant_idx] = antenna_motion;
                }
            }

            cache.push(SkdRowStatus {
                start_geometry,
                end_geometry,
                motion_1: motion_values[0].clone(),
                motion_2: motion_values[1].clone(),
            });
        }

        self.skd_status_cache = cache;
        self.skd_status_dirty = false;
    }

    fn find_source(&self, name: &str) -> Option<&Source> {
        self.sources
            .iter()
            .map(|(source, _)| source)
            .find(|source| source.name == name)
    }

    fn record_skd_undo(&mut self) {
        let snapshot = self.skd_rows.clone();
        if self.skd_undo_stack.last() != Some(&snapshot) {
            self.skd_undo_stack.push(snapshot);
            if self.skd_undo_stack.len() > SKD_HISTORY_LIMIT {
                self.skd_undo_stack.remove(0);
            }
        }
        self.skd_redo_stack.clear();
    }

    fn record_skd_undo_snapshot(&mut self, snapshot: Vec<SkdRow>) {
        if snapshot == self.skd_rows {
            return;
        }
        if self.skd_undo_stack.last() != Some(&snapshot) {
            self.skd_undo_stack.push(snapshot);
            if self.skd_undo_stack.len() > SKD_HISTORY_LIMIT {
                self.skd_undo_stack.remove(0);
            }
        }
        self.skd_redo_stack.clear();
    }

    fn clear_skd_selection(&mut self) {
        self.selected_skd_row = None;
        self.selected_skd_rows.clear();
    }

    fn select_only_skd_row(&mut self, index: usize) {
        self.selected_skd_rows.clear();
        if index < self.skd_rows.len() {
            self.selected_skd_rows.insert(index);
            self.selected_skd_row = Some(index);
        } else {
            self.selected_skd_row = None;
        }
    }

    fn select_skd_row(&mut self, index: usize, modifiers: egui::Modifiers) {
        if index >= self.skd_rows.len() {
            return;
        }
        if modifiers.shift {
            let anchor = self.selected_skd_row.unwrap_or(index);
            if !(modifiers.ctrl || modifiers.command) {
                self.selected_skd_rows.clear();
            }
            for row_index in anchor.min(index)..=anchor.max(index) {
                self.selected_skd_rows.insert(row_index);
            }
        } else if modifiers.ctrl || modifiers.command {
            if !self.selected_skd_rows.remove(&index) {
                self.selected_skd_rows.insert(index);
            }
        } else {
            self.selected_skd_rows.clear();
            self.selected_skd_rows.insert(index);
        }
        self.selected_skd_row = self
            .selected_skd_rows
            .contains(&index)
            .then_some(index)
            .or_else(|| self.selected_skd_rows.iter().next_back().copied());
    }

    fn selected_skd_indices(&self) -> Vec<usize> {
        self.selected_skd_rows
            .iter()
            .copied()
            .filter(|index| *index < self.skd_rows.len())
            .collect()
    }

    fn undo_skd_edit(&mut self) -> Result<(), String> {
        let previous = self
            .skd_undo_stack
            .pop()
            .ok_or_else(|| "Nothing to undo.".to_string())?;
        self.skd_redo_stack.push(self.skd_rows.clone());
        self.skd_rows = previous;
        self.clear_skd_selection();
        self.mark_skd_status_dirty();
        Ok(())
    }

    fn redo_skd_edit(&mut self) -> Result<(), String> {
        let next = self
            .skd_redo_stack
            .pop()
            .ok_or_else(|| "Nothing to redo.".to_string())?;
        self.skd_undo_stack.push(self.skd_rows.clone());
        self.skd_rows = next;
        self.clear_skd_selection();
        self.mark_skd_status_dirty();
        Ok(())
    }

    fn new_skd_row_from_inputs(&self) -> Result<SkdRow, String> {
        let source = self
            .sources
            .get(self.new_skd_source_index)
            .ok_or_else(|| "Load Source Settings before adding a row.".to_string())?;
        let start_time = normalize_time_string(&self.new_skd_start_time)
            .ok_or_else(|| "Start time must be HH:MM:SS or HHMMSS.".to_string())?;

        Ok(SkdRow {
            source_name: source.0.name.clone(),
            start_date: self.new_skd_start_date,
            start_time,
            duration_sec: self.new_skd_duration_sec,
            az_offset_deg: 0.0,
            el_offset_deg: 0.0,
            ra_offset_deg: 0.0,
            dec_offset_deg: 0.0,
            include_station_offsets: false,
        })
    }

    fn insert_new_skd_row(&mut self, index: usize) -> Result<(), String> {
        let row = self.new_skd_row_from_inputs()?;
        let index = index.min(self.skd_rows.len());
        self.record_skd_undo();
        self.skd_rows.insert(index, row);
        self.select_only_skd_row(index);
        self.mark_skd_status_dirty();
        Ok(())
    }

    fn duplicate_selected_skd_row(&mut self) -> Result<(), String> {
        let selected = self.selected_skd_indices();
        let insert_at = selected
            .last()
            .copied()
            .map(|index| index + 1)
            .ok_or_else(|| "Select one or more schedule rows first.".to_string())?;
        let rows: Vec<SkdRow> = selected
            .iter()
            .map(|index| self.skd_rows[*index].clone())
            .collect();
        self.record_skd_undo();
        let count = rows.len();
        self.skd_rows.splice(insert_at..insert_at, rows);
        self.selected_skd_rows = (insert_at..insert_at + count).collect();
        self.selected_skd_row = Some(insert_at + count - 1);
        self.mark_skd_status_dirty();
        Ok(())
    }

    fn move_selected_skd_row(&mut self, direction: isize) -> Result<(), String> {
        let selected = self.selected_skd_indices();
        if selected.is_empty() {
            return Err("Select one or more schedule rows first.".to_string());
        }
        if (direction < 0 && selected[0] == 0)
            || (direction > 0 && *selected.last().unwrap() + 1 >= self.skd_rows.len())
        {
            return Err("The selected block cannot be moved farther.".to_string());
        }

        self.record_skd_undo();
        if direction < 0 {
            for index in &selected {
                self.skd_rows.swap(*index, *index - 1);
            }
        } else {
            for index in selected.iter().rev() {
                self.skd_rows.swap(*index, *index + 1);
            }
        }
        self.selected_skd_rows = selected
            .iter()
            .map(|index| (*index as isize + direction) as usize)
            .collect();
        self.selected_skd_row = self
            .selected_skd_row
            .map(|index| (index as isize + direction) as usize);
        self.mark_skd_status_dirty();
        Ok(())
    }

    fn set_selected_start_after_previous(&mut self) -> Result<(), String> {
        let index = self
            .selected_skd_row
            .filter(|index| *index < self.skd_rows.len())
            .ok_or_else(|| "Select a schedule row first.".to_string())?;
        if index == 0 {
            return Err("The first row has no previous scan.".to_string());
        }

        let previous = &self.skd_rows[index - 1];
        let offset_sec = previous.duration_sec as i64 + self.skd_slew_gap_sec as i64;
        let (start_date, start_time) =
            offset_schedule_time(previous.start_date, &previous.start_time, offset_sec)?;
        self.record_skd_undo();
        self.skd_rows[index].start_date = start_date;
        self.skd_rows[index].start_time = start_time;
        self.mark_skd_status_dirty();
        Ok(())
    }

    fn delete_skd_row(&mut self, index: usize) {
        if index >= self.skd_rows.len() {
            return;
        }
        self.record_skd_undo();
        self.skd_rows.remove(index);
        if self.skd_rows.is_empty() {
            self.clear_skd_selection();
        } else {
            self.select_only_skd_row(index.min(self.skd_rows.len() - 1));
        }
        self.mark_skd_status_dirty();
    }

    fn delete_selected_skd_rows(&mut self) -> Result<(), String> {
        let selected = self.selected_skd_indices();
        if selected.is_empty() {
            return Err("Select one or more schedule rows first.".to_string());
        }
        self.record_skd_undo();
        let selected: BTreeSet<usize> = selected.into_iter().collect();
        self.skd_rows = self
            .skd_rows
            .drain(..)
            .enumerate()
            .filter_map(|(index, row)| (!selected.contains(&index)).then_some(row))
            .collect();
        self.clear_skd_selection();
        self.mark_skd_status_dirty();
        Ok(())
    }

    fn repeat_selected_skd_block(&mut self) -> Result<(), String> {
        let selected = self.selected_skd_indices();
        if selected.is_empty() {
            return Err("Select one or more rows for the observation block.".to_string());
        }
        if self.skd_block_repeat_count == 0 {
            return Err("Repeat count must be at least 1.".to_string());
        }

        let block: Vec<SkdRow> = selected
            .iter()
            .map(|index| self.skd_rows[*index].clone())
            .collect();
        let first_start = schedule_datetime(block[0].start_date, &block[0].start_time)?;
        let mut relative_starts = Vec::with_capacity(block.len());
        let mut block_span_sec = 0_i64;
        for row in &block {
            let row_start = schedule_datetime(row.start_date, &row.start_time)?;
            let relative_sec = (row_start - first_start).num_seconds();
            if relative_sec < 0 {
                return Err("Block rows must be in chronological order.".to_string());
            }
            relative_starts.push(relative_sec);
            block_span_sec = block_span_sec.max(relative_sec + row.duration_sec as i64);
        }

        let first_copy_start =
            first_start + Duration::seconds(block_span_sec + self.skd_block_gap_sec as i64);
        let insert_at = selected.last().copied().unwrap() + 1;
        let mut repeated = Vec::new();
        for repeat_index in 0..self.skd_block_repeat_count {
            let repeat_start = first_copy_start
                + Duration::seconds(
                    repeat_index as i64 * (block_span_sec + self.skd_block_gap_sec as i64),
                );
            for (row, relative_sec) in block.iter().zip(&relative_starts) {
                let mut copy = row.clone();
                set_skd_row_datetime(&mut copy, repeat_start + Duration::seconds(*relative_sec));
                repeated.push(copy);
            }
        }

        self.record_skd_undo();
        let repeated_count = repeated.len();
        self.skd_rows.splice(insert_at..insert_at, repeated);
        self.selected_skd_rows = (insert_at..insert_at + repeated_count).collect();
        self.selected_skd_row = Some(insert_at + repeated_count - 1);
        self.mark_skd_status_dirty();
        Ok(())
    }

    fn copy_selected_skd_block(&mut self) -> Result<(), String> {
        let selected = self.selected_skd_indices();
        if selected.is_empty() {
            return Err("Select one or more rows for the observation block.".to_string());
        }
        self.skd_block_clipboard = selected
            .iter()
            .map(|index| self.skd_rows[*index].clone())
            .collect();
        Ok(())
    }

    fn paste_skd_block(&mut self, insert_at: usize) -> Result<(), String> {
        if self.skd_block_clipboard.is_empty() {
            return Err("Copy an observation block first.".to_string());
        }
        let first_start = schedule_datetime(
            self.skd_block_clipboard[0].start_date,
            &self.skd_block_clipboard[0].start_time,
        )?;
        let mut relative_starts = Vec::with_capacity(self.skd_block_clipboard.len());
        let mut block_span_sec = 0_i64;
        for row in &self.skd_block_clipboard {
            let row_start = schedule_datetime(row.start_date, &row.start_time)?;
            let relative_sec = (row_start - first_start).num_seconds();
            if relative_sec < 0 {
                return Err("Block rows must be in chronological order.".to_string());
            }
            relative_starts.push(relative_sec);
            block_span_sec = block_span_sec.max(relative_sec + row.duration_sec as i64);
        }

        let insert_at = insert_at.min(self.skd_rows.len());
        let paste_start = if insert_at > 0 {
            let previous = &self.skd_rows[insert_at - 1];
            schedule_datetime(previous.start_date, &previous.start_time)?
                + Duration::seconds(previous.duration_sec as i64 + self.skd_block_gap_sec as i64)
        } else if let Some(next) = self.skd_rows.first() {
            schedule_datetime(next.start_date, &next.start_time)?
                - Duration::seconds(block_span_sec + self.skd_block_gap_sec as i64)
        } else {
            first_start
        };
        let mut pasted = self.skd_block_clipboard.clone();
        for (row, relative_sec) in pasted.iter_mut().zip(relative_starts) {
            set_skd_row_datetime(row, paste_start + Duration::seconds(relative_sec));
        }

        self.record_skd_undo();
        let count = pasted.len();
        self.skd_rows.splice(insert_at..insert_at, pasted);
        self.selected_skd_rows = (insert_at..insert_at + count).collect();
        self.selected_skd_row = Some(insert_at + count - 1);
        self.mark_skd_status_dirty();
        Ok(())
    }

    fn apply_bulk_source(&mut self) -> Result<(), String> {
        let selected = self.selected_skd_indices();
        let source_name = self
            .sources
            .get(self.skd_bulk_source_index)
            .map(|source| source.0.name.clone())
            .ok_or_else(|| "Load Source Settings first.".to_string())?;
        if selected.is_empty() {
            return Err("Select one or more schedule rows first.".to_string());
        }
        self.record_skd_undo();
        for index in selected {
            self.skd_rows[index].source_name = source_name.clone();
        }
        self.mark_skd_status_dirty();
        Ok(())
    }

    fn apply_bulk_duration(&mut self) -> Result<(), String> {
        let selected = self.selected_skd_indices();
        if selected.is_empty() {
            return Err("Select one or more schedule rows first.".to_string());
        }
        self.record_skd_undo();
        for index in selected {
            self.skd_rows[index].duration_sec = self.skd_bulk_duration_sec;
        }
        self.mark_skd_status_dirty();
        Ok(())
    }

    fn apply_bulk_offsets(&mut self) -> Result<(), String> {
        let selected = self.selected_skd_indices();
        if selected.is_empty() {
            return Err("Select one or more schedule rows first.".to_string());
        }
        self.record_skd_undo();
        for index in selected {
            let row = &mut self.skd_rows[index];
            row.az_offset_deg = self.skd_bulk_az_offset_deg;
            row.el_offset_deg = self.skd_bulk_el_offset_deg;
            row.ra_offset_deg = self.skd_bulk_ra_offset_deg;
            row.dec_offset_deg = self.skd_bulk_dec_offset_deg;
        }
        self.mark_skd_status_dirty();
        Ok(())
    }

    fn apply_bulk_time_shift(&mut self) -> Result<(), String> {
        let selected = self.selected_skd_indices();
        if selected.is_empty() {
            return Err("Select one or more schedule rows first.".to_string());
        }
        let mut updated = self.skd_rows.clone();
        for index in selected {
            let start = schedule_datetime(updated[index].start_date, &updated[index].start_time)?;
            set_skd_row_datetime(
                &mut updated[index],
                start + Duration::seconds(self.skd_bulk_shift_sec as i64),
            );
        }
        self.record_skd_undo();
        self.skd_rows = updated;
        self.mark_skd_status_dirty();
        Ok(())
    }

    fn auto_schedule_from_selected(&mut self) -> Result<(), String> {
        if self.skd_rows.len() < 2 {
            return Err("At least two schedule rows are required.".to_string());
        }
        let start_index = self
            .selected_skd_row
            .filter(|index| *index < self.skd_rows.len())
            .unwrap_or(1)
            .max(1);

        let mut antenna_indices = Vec::new();
        for index in [self.selected_antenna, self.selected_antenna_2] {
            if index < self.antennas.len() && !antenna_indices.contains(&index) {
                antenna_indices.push(index);
            }
        }
        let antennas: Vec<Antenna> = antenna_indices
            .iter()
            .filter_map(|index| self.antennas.get(*index).cloned())
            .collect();
        if antennas.is_empty() {
            return Err("Load and select at least one antenna first.".to_string());
        }
        let source_map: HashMap<String, Source> = self
            .sources
            .iter()
            .map(|(source, _)| (source.name.clone(), source.clone()))
            .collect();

        let mut updated = self.skd_rows.clone();
        for index in start_index..updated.len() {
            let previous = updated[index - 1].clone();
            let previous_source = source_map
                .get(&previous.source_name)
                .ok_or_else(|| format!("Source '{}' is not loaded.", previous.source_name))?;
            let current_source_name = updated[index].source_name.clone();
            let current_source = source_map
                .get(&current_source_name)
                .ok_or_else(|| format!("Source '{}' is not loaded.", current_source_name))?;
            let previous_end = schedule_datetime(previous.start_date, &previous.start_time)?
                + Duration::seconds(previous.duration_sec as i64);
            let mut gap_sec = self.skd_auto_slew_margin_sec as i64;
            let mut converged = false;

            for _ in 0..32 {
                let candidate_start = previous_end + Duration::seconds(gap_sec);
                let mut candidate = updated[index].clone();
                set_skd_row_datetime(&mut candidate, candidate_start);
                let mut required_sec = 0_i64;
                for antenna in &antennas {
                    let (_, previous_az, previous_el) =
                        scan_az_el_for(&previous, previous_source, antenna.pos, true).ok_or_else(
                            || format!("Cannot calculate the end position of row {}.", index),
                        )?;
                    let (_, current_az, current_el) =
                        scan_az_el_for(&candidate, current_source, antenna.pos, false).ok_or_else(
                            || format!("Cannot calculate the start position of row {}.", index + 1),
                        )?;
                    let antenna_slew = antenna
                        .slew_seconds(previous_az, previous_el, current_az, current_el)
                        .ok_or_else(|| {
                            format!(
                                "Row {} is outside the limits of antenna {}.",
                                index + 1,
                                antenna.name
                            )
                        })?
                        .ceil() as i64
                        + self.skd_auto_slew_margin_sec as i64;
                    required_sec = required_sec.max(antenna_slew);
                }
                if gap_sec >= required_sec {
                    set_skd_row_datetime(&mut updated[index], candidate_start);
                    converged = true;
                    break;
                }
                gap_sec = required_sec;
            }
            if !converged {
                return Err(format!(
                    "Slew calculation did not converge for row {}.",
                    index + 1
                ));
            }
        }

        self.record_skd_undo();
        self.skd_rows = updated;
        self.mark_skd_status_dirty();
        Ok(())
    }

    fn skd_problem_indices(&self) -> Vec<usize> {
        self.skd_status_cache
            .iter()
            .enumerate()
            .filter_map(|(index, status)| skd_status_has_problem(status).then_some(index))
            .collect()
    }

    fn required_slew_between_rows(&self, previous: &SkdRow, current: &SkdRow) -> Option<f64> {
        let previous_source = self.find_source(&previous.source_name)?;
        let current_source = self.find_source(&current.source_name)?;
        let mut antenna_indices = Vec::new();
        for index in [self.selected_antenna, self.selected_antenna_2] {
            if index < self.antennas.len() && !antenna_indices.contains(&index) {
                antenna_indices.push(index);
            }
        }
        let mut required_sec = 0.0_f64;
        let mut found_antenna = false;
        for index in antenna_indices {
            found_antenna = true;
            let antenna = &self.antennas[index];
            let (_, previous_az, previous_el) =
                scan_az_el_for(previous, previous_source, antenna.pos, true)?;
            let (_, current_az, current_el) =
                scan_az_el_for(current, current_source, antenna.pos, false)?;
            required_sec = required_sec.max(antenna.slew_seconds(
                previous_az,
                previous_el,
                current_az,
                current_el,
            )?);
        }
        found_antenna.then_some(required_sec)
    }

    fn show_skd_timeline(&self, ui: &mut egui::Ui) {
        let dated_rows: Vec<(usize, chrono::NaiveDateTime)> = self
            .skd_rows
            .iter()
            .enumerate()
            .filter_map(|(index, row)| {
                schedule_datetime(row.start_date, &row.start_time)
                    .ok()
                    .map(|datetime| (index, datetime))
            })
            .collect();
        let Some(base_time) = dated_rows.iter().map(|(_, datetime)| *datetime).min() else {
            ui.label("No valid scan times for the timeline.");
            return;
        };

        let mut scan_bars = Vec::with_capacity(dated_rows.len());
        let mut slew_bars = Vec::new();
        let mut idle_bars = Vec::new();
        let mut max_minute = 1.0_f64;
        for (index, start) in dated_rows {
            let row = &self.skd_rows[index];
            // Negative coordinates keep scan #1 at the top while preserving
            // the visible row number on an upward-increasing plot axis.
            let row_center = -(index as f64 + 1.0);
            let observation_y = row_center + 0.20;
            let transition_y = row_center - 0.20;
            let start_minute = (start - base_time).num_seconds() as f64 / 60.0;
            let duration_minute = row.duration_sec as f64 / 60.0;
            max_minute = max_minute.max(start_minute + duration_minute);
            let has_problem = self
                .skd_status_cache
                .get(index)
                .is_some_and(skd_status_has_problem);
            let fill = if has_problem {
                egui::Color32::from_rgb(190, 55, 55)
            } else if self.selected_skd_rows.contains(&index) {
                egui::Color32::from_rgb(60, 170, 90)
            } else {
                egui::Color32::from_rgb(70, 125, 200)
            };
            scan_bars.push(
                Bar::new(observation_y, duration_minute)
                    .horizontal()
                    .base_offset(start_minute)
                    .width(0.32)
                    .fill(fill)
                    .name(format!(
                        "#{} {}  {}  {}s",
                        index + 1,
                        row.source_name,
                        start.format("%Y-%m-%d %H:%M:%S"),
                        row.duration_sec
                    )),
            );

            if index > 0 {
                let previous = &self.skd_rows[index - 1];
                if let Ok(previous_start) =
                    schedule_datetime(previous.start_date, &previous.start_time)
                {
                    let previous_end =
                        previous_start + Duration::seconds(previous.duration_sec as i64);
                    let available_sec = (start - previous_end).num_seconds() as f64;
                    let previous_end_minute =
                        (previous_end - base_time).num_seconds() as f64 / 60.0;
                    if available_sec < 0.0 {
                        idle_bars.push(
                            Bar::new(transition_y, -available_sec / 60.0)
                                .horizontal()
                                .base_offset(start_minute)
                                .width(0.32)
                                .fill(egui::Color32::RED)
                                .name(format!(
                                    "Overlap before row {}: {:.0}s",
                                    index + 1,
                                    -available_sec
                                )),
                        );
                    } else if let Some(required_sec) =
                        self.required_slew_between_rows(previous, row)
                    {
                        let slew_color = if required_sec <= available_sec + 1.0e-6 {
                            egui::Color32::from_rgb(225, 145, 35)
                        } else {
                            egui::Color32::RED
                        };
                        slew_bars.push(
                            Bar::new(transition_y, required_sec / 60.0)
                                .horizontal()
                                .base_offset(previous_end_minute)
                                .width(0.32)
                                .fill(slew_color)
                                .name(format!(
                                    "Slew to row {}: required {:.0}s / available {:.0}s",
                                    index + 1,
                                    required_sec.ceil(),
                                    available_sec
                                )),
                        );
                        if available_sec > required_sec {
                            idle_bars.push(
                                Bar::new(transition_y, (available_sec - required_sec) / 60.0)
                                    .horizontal()
                                    .base_offset(previous_end_minute + required_sec / 60.0)
                                    .width(0.32)
                                    .fill(egui::Color32::from_gray(95))
                                    .name(format!(
                                        "Idle before row {}: {:.0}s",
                                        index + 1,
                                        available_sec - required_sec
                                    )),
                            );
                        }
                    } else if available_sec > 0.0 {
                        idle_bars.push(
                            Bar::new(transition_y, available_sec / 60.0)
                                .horizontal()
                                .base_offset(previous_end_minute)
                                .width(0.32)
                                .fill(egui::Color32::from_gray(95))
                                .name(format!(
                                    "Unclassified gap before row {}: {:.0}s",
                                    index + 1,
                                    available_sec
                                )),
                        );
                    }
                }
            }
        }

        let scan_chart = BarChart::new("Observation", scan_bars)
            .horizontal()
            .element_formatter(Box::new(|bar, _| bar.name.clone()));
        let slew_chart = BarChart::new("Slew", slew_bars)
            .horizontal()
            .element_formatter(Box::new(|bar, _| bar.name.clone()));
        let idle_chart = BarChart::new("Idle / overlap", idle_bars)
            .horizontal()
            .element_formatter(Box::new(|bar, _| bar.name.clone()));
        let row_labels: Vec<String> = self
            .skd_rows
            .iter()
            .enumerate()
            .map(|(index, row)| format!("#{} {}", index + 1, row.source_name))
            .collect();
        Plot::new("skd_timeline_plot")
            .height((80.0 + self.skd_rows.len() as f32 * 10.0).clamp(170.0, 400.0))
            .include_x(0.0)
            .include_x(max_minute)
            .include_y(-(self.skd_rows.len() as f64 + 1.0))
            .include_y(0.0)
            .show_y(true)
            .y_axis_min_width(150.0)
            .y_axis_formatter(move |mark, _| {
                let row_number = (-mark.value).round() as usize;
                if row_number > 0
                    && row_number <= row_labels.len()
                    && ((-mark.value) - row_number as f64).abs() < 0.1
                {
                    row_labels[row_number - 1].clone()
                } else {
                    String::new()
                }
            })
            .x_axis_label("UTC")
            .x_axis_formatter(move |mark, _| {
                let datetime = base_time + Duration::seconds((mark.value * 60.0).round() as i64);
                datetime.format("%m-%d %H:%M").to_string()
            })
            .coordinates_formatter(
                Corner::LeftTop,
                egui_plot::CoordinatesFormatter::new(move |point, _| {
                    let datetime = base_time + Duration::seconds((point.x * 60.0).round() as i64);
                    datetime.format("%Y-%m-%d %H:%M:%S UTC").to_string()
                }),
            )
            .legend(Legend::default())
            .show(ui, |plot_ui| {
                plot_ui.bar_chart(idle_chart);
                plot_ui.bar_chart(slew_chart);
                plot_ui.bar_chart(scan_chart);
            });
    }

    fn ui_skd_table_tab(&mut self, ui: &mut egui::Ui) {
        let undo_shortcut = ui.input(|input| {
            input.modifiers.command && !input.modifiers.shift && input.key_pressed(egui::Key::Z)
        });
        let redo_shortcut = ui.input(|input| {
            input.modifiers.command
                && (input.key_pressed(egui::Key::Y)
                    || (input.modifiers.shift && input.key_pressed(egui::Key::Z)))
        });
        if undo_shortcut {
            if let Err(error) = self.undo_skd_edit() {
                self.error_msg = Some(error);
            }
        } else if redo_shortcut {
            if let Err(error) = self.redo_skd_edit() {
                self.error_msg = Some(error);
            }
        }

        ui.horizontal(|ui| {
            let setup_label = if self.skd_setup_open {
                "Hide setup panel"
            } else {
                "Show setup panel"
            };
            if ui.button(setup_label).clicked() {
                self.skd_setup_open = !self.skd_setup_open;
            }
            ui.separator();
            ui.strong(format!("{} scans", self.skd_rows.len()));
            if ui.button("Sort by start time").clicked() {
                self.record_skd_undo();
                self.sort_skd_rows_by_start_time();
            }
            ui.separator();
            if ui
                .add_enabled(!self.skd_undo_stack.is_empty(), egui::Button::new("Undo"))
                .on_hover_text("Undo schedule edit (Ctrl+Z)")
                .clicked()
            {
                if let Err(error) = self.undo_skd_edit() {
                    self.error_msg = Some(error);
                }
            }
            if ui
                .add_enabled(!self.skd_redo_stack.is_empty(), egui::Button::new("Redo"))
                .on_hover_text("Redo schedule edit (Ctrl+Y)")
                .clicked()
            {
                if let Err(error) = self.redo_skd_edit() {
                    self.error_msg = Some(error);
                }
            }
        });
        ui.separator();

        let available = ui.available_size();
        let left_width = (available.x * 0.34).clamp(520.0, 580.0);
        let right_width = if self.skd_setup_open {
            (available.x - left_width - 12.0).max(500.0)
        } else {
            available.x
        };
        let parameter_panel_width = (left_width - 28.0).max(280.0);
        ui.horizontal(|ui| {
            if self.skd_setup_open {
                ui.allocate_ui_with_layout(
                    egui::vec2(left_width, available.y),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        egui::ScrollArea::vertical()
                            .id_salt("skd_left_parameters")
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                ui.group(|ui| {
                                    ui.set_min_width(parameter_panel_width);
                                    ui.set_max_width(parameter_panel_width);
                                    ui.strong("Input DRG");
                                    ui.horizontal_wrapped(|ui| {
                                        if ui.button("Load DRG").clicked() {
                                            match pick_file_dialog("Select DRG file") {
                                                Ok(Some(path)) => {
                                                    self.input_drg_file_path =
                                                        path.to_string_lossy().to_string();
                                                    match self.load_drg_file() {
                                                        Ok(_) => {
                                                            self.error_msg =
                                                                Some("Loaded DRG.".to_string())
                                                        }
                                                        Err(e) => self.error_msg = Some(e),
                                                    }
                                                }
                                                Ok(None) => {}
                                                Err(e) => self.error_msg = Some(e),
                                            }
                                        }
                                        if ui.button("Open Input").clicked() {
                                            let input_path =
                                                output_drg_path(&self.input_drg_file_path)
                                                    .map(|path| path.to_string_lossy().to_string());
                                            match input_path.and_then(|path| {
                                                utils::open_file_in_external_editor(&path)
                                            }) {
                                                Ok(_) => self.error_msg = None,
                                                Err(e) => self.error_msg = Some(e),
                                            }
                                        }
                                        let input_drg_path_text =
                                            if self.input_drg_file_path.is_empty() {
                                                "<DRG path>"
                                            } else {
                                                self.input_drg_file_path.as_str()
                                            };
                                        ui.add_sized(
                                            [parameter_panel_width, 20.0],
                                            egui::Label::new(
                                                egui::RichText::new(input_drg_path_text)
                                                    .monospace(),
                                            )
                                            .truncate(),
                                        )
                                        .on_hover_text(input_drg_path_text);
                                    });
                                });

                                ui.group(|ui| {
                                    ui.set_min_width(parameter_panel_width);
                                    ui.set_max_width(parameter_panel_width);
                                    ui.strong("Output");
                                    ui.horizontal_wrapped(|ui| {
                                        ui.label("Obscode:");
                                        ui.add_sized(
                                            [120.0, 20.0],
                                            egui::TextEdit::singleline(&mut self.obs_code),
                                        );
                                        ui.label("PI:");
                                        ui.add_sized(
                                            [120.0, 20.0],
                                            egui::TextEdit::singleline(&mut self.pi_name),
                                        );
                                    });
                                    ui.horizontal_wrapped(|ui| {
                                        if ui.button("Create DRG").clicked() {
                                            match self.write_skd_to_drg() {
                                                Ok(paths) => {
                                                    self.error_msg = Some(format!(
                                                        "Created {}",
                                                        paths
                                                            .iter()
                                                            .map(|path| path.display().to_string())
                                                            .collect::<Vec<_>>()
                                                            .join(", ")
                                                    ))
                                                }
                                                Err(e) => self.error_msg = Some(e),
                                            }
                                        }
                                        if ui.button("Open DRG").clicked() {
                                            let output_path = self
                                                .obs_code_output_path(self.obs_code.trim())
                                                .map(|path| path.to_string_lossy().to_string());
                                            match output_path.and_then(|path| {
                                                utils::open_file_in_external_editor(&path)
                                            }) {
                                                Ok(_) => self.error_msg = None,
                                                Err(e) => self.error_msg = Some(e),
                                            }
                                        }
                                        match (
                                            self.obs_code_output_path(self.obs_code.trim()),
                                            self.obs_code_station_skd_output_path(
                                                self.obs_code.trim(),
                                                "32",
                                            ),
                                            self.obs_code_station_skd_output_path(
                                                self.obs_code.trim(),
                                                "34",
                                            ),
                                        ) {
                                            (Ok(drg_path), Ok(skd32_path), Ok(skd34_path)) => {
                                                let output_path_text = format!(
                                                    "{} | {} | {}",
                                                    drg_path.display(),
                                                    skd32_path.display(),
                                                    skd34_path.display()
                                                );
                                                ui.add_sized(
                                                    [parameter_panel_width, 20.0],
                                                    egui::Label::new(
                                                        egui::RichText::new(
                                                            output_path_text.as_str(),
                                                        )
                                                        .monospace(),
                                                    )
                                                    .truncate(),
                                                )
                                                .on_hover_text(output_path_text);
                                            }
                                            _ => {
                                                ui.add_sized(
                                                    [parameter_panel_width, 20.0],
                                                    egui::Label::new(
                                                        egui::RichText::new("<DRG/SKD>")
                                                            .monospace(),
                                                    )
                                                    .truncate(),
                                                );
                                            }
                                        }
                                    });
                                });

                                ui.group(|ui| {
                                    ui.set_min_width(parameter_panel_width);
                                    ui.set_max_width(parameter_panel_width);
                                    ui.strong("Antenna SCH");
                                    if self.antennas.is_empty() {
                                        ui.label("No antenna loaded");
                                    } else {
                                        ui.label(format!(
                                            "{} antennas loaded",
                                            self.antennas.len()
                                        ));
                                        self.selected_antenna =
                                            self.selected_antenna.min(self.antennas.len() - 1);
                                        self.selected_antenna_2 =
                                            self.selected_antenna_2.min(self.antennas.len() - 1);
                                        let old_selected_antenna = self.selected_antenna;
                                        let old_selected_antenna_2 = self.selected_antenna_2;
                                        for (selected, combo_id) in [
                                            (&mut self.selected_antenna, "skd_antenna_1"),
                                            (&mut self.selected_antenna_2, "skd_antenna_2"),
                                        ] {
                                            ui.horizontal_wrapped(|ui| {
                                                egui::ComboBox::from_id_salt(combo_id)
                                                    .selected_text(&self.antennas[*selected].name)
                                                    .show_ui(ui, |ui| {
                                                        for (i, antenna) in
                                                            self.antennas.iter().enumerate()
                                                        {
                                                            ui.selectable_value(
                                                                selected,
                                                                i,
                                                                format!(
                                                                    "{} {}",
                                                                    antenna.code, antenna.name
                                                                ),
                                                            );
                                                        }
                                                    });
                                                let antenna = &self.antennas[*selected];
                                                ui.label(format!(
                                        "AZ {:.1}-{:.1} {:.1}/min, EL {:.1}-{:.1} {:.1}/min",
                                        antenna.az_min_deg,
                                        antenna.az_max_deg,
                                        antenna.az_rate_deg_per_min,
                                        antenna.el_min_deg,
                                        antenna.el_max_deg,
                                        antenna.el_rate_deg_per_min
                                    ));
                                            });
                                        }
                                        if self.selected_antenna != old_selected_antenna
                                            || self.selected_antenna_2 != old_selected_antenna_2
                                        {
                                            self.mark_skd_status_dirty();
                                        }
                                    }
                                });

                                ui.group(|ui| {
                                    ui.set_min_width(parameter_panel_width);
                                    ui.set_max_width(parameter_panel_width);
                                    ui.strong("Schedule Date");
                                    if ui
                                        .button(self.selected_date.format("%Y-%m-%d").to_string())
                                        .clicked()
                                    {
                                        self.show_calendar = !self.show_calendar;
                                    }
                                });

                                ui.group(|ui| {
                                    ui.set_min_width(parameter_panel_width);
                                    ui.set_max_width(parameter_panel_width);
                                    ui.strong("Time Shift");
                                    ui.horizontal(|ui| {
                                        ui.add(
                                            egui::DragValue::new(&mut self.schedule_time_shift_sec)
                                                .speed(1)
                                                .suffix(" s"),
                                        );
                                        if ui.button("Apply to All").clicked() {
                                            match self.apply_schedule_time_shift() {
                                                Ok(_) => {
                                                    self.error_msg = Some(format!(
                                                        "Shifted all scans by {} seconds.",
                                                        self.schedule_time_shift_sec
                                                    ))
                                                }
                                                Err(e) => self.error_msg = Some(e),
                                            }
                                        }
                                    });
                                });

                                if self.sources.is_empty() {
                                    ui.label("Load source.txt first");
                                } else {
                                    ui.group(|ui| {
                                        ui.set_min_width(parameter_panel_width);
                                        ui.set_max_width(parameter_panel_width);
                                        self.interleave_target_index = self
                                            .interleave_target_index
                                            .min(self.sources.len() - 1);
                                        self.interleave_cal_index =
                                            self.interleave_cal_index.min(self.sources.len() - 1);
                                        ui.strong("Target / Gain Calibrator");
                                        ui.add_space(4.0);
                                        egui::Grid::new("interleave_source_grid")
                                            .num_columns(2)
                                            .spacing([12.0, 6.0])
                                            .show(ui, |ui| {
                                                ui.label("Target");
                                                show_source_search_selector(
                                                    ui,
                                                    "interleave_target",
                                                    &self.sources,
                                                    &mut self.interleave_target_index,
                                                    &mut self.target_picker_filter,
                                                );
                                                ui.end_row();

                                                ui.label("Gain Cal");
                                                show_source_search_selector(
                                                    ui,
                                                    "interleave_cal",
                                                    &self.sources,
                                                    &mut self.interleave_cal_index,
                                                    &mut self.cal_picker_filter,
                                                );
                                                ui.end_row();
                                            });
                                        ui.separator();
                                        egui::Grid::new("interleave_parameter_grid")
                                            .num_columns(4)
                                            .spacing([12.0, 6.0])
                                            .show(ui, |ui| {
                                                ui.label("Target Duration");
                                                ui.add(
                                                    egui::DragValue::new(
                                                        &mut self.interleave_target_duration_sec,
                                                    )
                                                    .speed(10)
                                                    .range(1..=86400)
                                                    .suffix(" s"),
                                                );
                                                ui.label("Cal Duration");
                                                ui.add(
                                                    egui::DragValue::new(
                                                        &mut self.interleave_cal_duration_sec,
                                                    )
                                                    .speed(10)
                                                    .range(1..=86400)
                                                    .suffix(" s"),
                                                );
                                                ui.end_row();

                                                ui.label("Start UT");
                                                ui.add_sized(
                                                    [92.0, 20.0],
                                                    egui::TextEdit::singleline(
                                                        &mut self.interleave_start_time,
                                                    ),
                                                );
                                                ui.label("Slew");
                                                ui.add(
                                                    egui::DragValue::new(
                                                        &mut self.interleave_slew_sec,
                                                    )
                                                    .speed(10)
                                                    .range(0..=86400)
                                                    .suffix(" s"),
                                                );
                                                ui.end_row();

                                                ui.label("Cycles");
                                                ui.add(
                                                    egui::DragValue::new(
                                                        &mut self.interleave_cycles,
                                                    )
                                                    .speed(1)
                                                    .range(1..=1000),
                                                );
                                                ui.checkbox(
                                                    &mut self.interleave_clear_existing,
                                                    "Replace all rows",
                                                )
                                                .on_hover_text(
                                                    "Delete the loaded/current schedule before generating.",
                                                );
                                                if ui.button("Generate").clicked() {
                                                    match self.generate_interleaved_skd_rows() {
                                                        Ok(_) => self.error_msg = None,
                                                        Err(e) => self.error_msg = Some(e),
                                                    }
                                                }
                                                ui.end_row();
                                            });
                                    });

                                    ui.group(|ui| {
                                        ui.set_min_width(parameter_panel_width);
                                        ui.set_max_width(parameter_panel_width);
                                        self.five_point_cal_index =
                                            self.five_point_cal_index.min(self.sources.len() - 1);
                                        ui.strong("Five-point Observation");
                                        ui.add_space(4.0);
                                        egui::Grid::new("five_point_source_grid")
                                            .num_columns(2)
                                            .spacing([12.0, 6.0])
                                            .show(ui, |ui| {
                                                ui.label("Gain Cal");
                                                show_source_search_selector(
                                                    ui,
                                                    "five_point_cal",
                                                    &self.sources,
                                                    &mut self.five_point_cal_index,
                                                    &mut self.five_point_picker_filter,
                                                );
                                                ui.end_row();
                                            });
                                        ui.separator();
                                        egui::Grid::new("five_point_parameter_grid")
                                            .num_columns(4)
                                            .spacing([12.0, 6.0])
                                            .show(ui, |ui| {
                                                ui.label("Start UT");
                                                ui.add_sized(
                                                    [92.0, 20.0],
                                                    egui::TextEdit::singleline(
                                                        &mut self.five_point_start_time,
                                                    ),
                                                );
                                                ui.label("Obs Duration");
                                                ui.add(
                                                    egui::DragValue::new(
                                                        &mut self.five_point_obstime_sec,
                                                    )
                                                    .speed(10)
                                                    .range(1..=86400)
                                                    .suffix(" s"),
                                                );
                                                ui.end_row();

                                                ui.label("Slew");
                                                ui.add(
                                                    egui::DragValue::new(
                                                        &mut self.five_point_slew_sec,
                                                    )
                                                    .speed(10)
                                                    .range(0..=86400)
                                                    .suffix(" s"),
                                                );
                                                ui.label("Offset");
                                                ui.add(
                                                    egui::DragValue::new(
                                                        &mut self.five_point_offset_deg,
                                                    )
                                                    .speed(0.1)
                                                    .suffix(" arcmin"),
                                                );
                                                ui.end_row();
                                            });
                                        ui.horizontal(|ui| {
                                            ui.checkbox(
                                                &mut self.five_point_include_station_offsets,
                                                "Include 32/34 +/-2 arcmin",
                                            );
                                            ui.checkbox(
                                                &mut self.five_point_clear_existing,
                                                "Replace all rows",
                                            )
                                            .on_hover_text(
                                                "Delete the loaded/current schedule before generating.",
                                            );
                                            if ui.button("Generate 10 Scans").clicked() {
                                                match self.generate_five_point_skd_rows() {
                                                    Ok(_) => self.error_msg = None,
                                                    Err(e) => self.error_msg = Some(e),
                                                }
                                            }
                                        });
                                    });

                                    ui.group(|ui| {
                                        ui.set_min_width(parameter_panel_width);
                                        ui.set_max_width(parameter_panel_width);
                                        ui.strong("New Row");
                                        self.new_skd_source_index =
                                            self.new_skd_source_index.min(self.sources.len() - 1);
                                        ui.add_space(4.0);
                                        egui::Grid::new("new_skd_source_grid")
                                            .num_columns(2)
                                            .spacing([12.0, 6.0])
                                            .show(ui, |ui| {
                                                ui.label("Source");
                                                show_source_search_selector(
                                                    ui,
                                                    "new_skd_source",
                                                    &self.sources,
                                                    &mut self.new_skd_source_index,
                                                    &mut self.new_skd_source_filter,
                                                );
                                                ui.end_row();
                                            });
                                        ui.separator();
                                        egui::Grid::new("new_skd_parameter_grid")
                                            .num_columns(4)
                                            .spacing([12.0, 6.0])
                                            .show(ui, |ui| {
                                                ui.label("Date");
                                                if ui
                                                    .button(
                                                        self.new_skd_start_date
                                                            .format("%Y-%m-%d")
                                                            .to_string(),
                                                    )
                                                    .clicked()
                                                {
                                                    self.show_new_skd_calendar =
                                                        !self.show_new_skd_calendar;
                                                }
                                                ui.label("Start UT");
                                                ui.add_sized(
                                                    [92.0, 20.0],
                                                    egui::TextEdit::singleline(
                                                        &mut self.new_skd_start_time,
                                                    ),
                                                );
                                                ui.end_row();

                                                ui.label("Duration");
                                                ui.add(
                                                    egui::DragValue::new(
                                                        &mut self.new_skd_duration_sec,
                                                    )
                                                    .speed(10)
                                                    .range(1..=86400)
                                                    .suffix(" s"),
                                                );
                                                ui.label("");
                                                ui.label("");
                                                ui.end_row();
                                            });
                                        let selected_index = self
                                            .selected_skd_row
                                            .filter(|index| *index < self.skd_rows.len());
                                        ui.small(match selected_index {
                                            Some(index) => format!(
                                                "Selected row: {} (click a row number to change)",
                                                index + 1
                                            ),
                                            None => "Select a row number to insert before/after"
                                                .to_string(),
                                        });
                                        let mut insertion_index = None;
                                        ui.horizontal_wrapped(|ui| {
                                            if ui.button("Append").clicked() {
                                                insertion_index = Some(self.skd_rows.len());
                                            }
                                            if ui
                                                .add_enabled(
                                                    selected_index.is_some(),
                                                    egui::Button::new("Insert Before"),
                                                )
                                                .clicked()
                                            {
                                                insertion_index = selected_index;
                                            }
                                            if ui
                                                .add_enabled(
                                                    selected_index.is_some(),
                                                    egui::Button::new("Insert After"),
                                                )
                                                .clicked()
                                            {
                                                insertion_index = selected_index.map(|index| index + 1);
                                            }
                                        });
                                        if let Some(index) = insertion_index {
                                            match self.insert_new_skd_row(index) {
                                                Ok(()) => self.error_msg = None,
                                                Err(error) => self.error_msg = Some(error),
                                            }
                                        }
                                    });
                                }
                            });
                    },
                );

                ui.separator();
            }
            ui.allocate_ui_with_layout(
                egui::vec2(right_width, available.y),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    self.selected_skd_rows
                        .retain(|index| *index < self.skd_rows.len());
                    if self
                        .selected_skd_row
                        .is_none_or(|index| !self.selected_skd_rows.contains(&index))
                    {
                        self.selected_skd_row =
                            self.selected_skd_rows.iter().next_back().copied();
                    }
                    let selected_index = self.selected_skd_row;
                    let selected_count = self.selected_skd_rows.len();
                    let mut row_action = None;
                    ui.horizontal_wrapped(|ui| {
                        ui.strong(if selected_count == 0 {
                            "Row actions".to_string()
                        } else {
                            format!("{} selected", selected_count)
                        });
                        if ui.button("Select All").clicked() {
                            self.selected_skd_rows = (0..self.skd_rows.len()).collect();
                            self.selected_skd_row = self.skd_rows.len().checked_sub(1);
                        }
                        if ui
                            .add_enabled(selected_count > 0, egui::Button::new("Clear"))
                            .clicked()
                        {
                            self.clear_skd_selection();
                        }
                        if ui
                            .add_enabled(
                                selected_count > 0,
                                egui::Button::new("Duplicate Block"),
                            )
                            .clicked()
                        {
                            row_action = Some("duplicate");
                        }
                        if ui
                            .add_enabled(
                                self.selected_skd_rows
                                    .iter()
                                    .next()
                                    .is_some_and(|index| *index > 0),
                                egui::Button::new("Move Block Up"),
                            )
                            .clicked()
                        {
                            row_action = Some("up");
                        }
                        if ui
                            .add_enabled(
                                self.selected_skd_rows
                                    .iter()
                                    .next_back()
                                    .is_some_and(|index| *index + 1 < self.skd_rows.len()),
                                egui::Button::new("Move Block Down"),
                            )
                            .clicked()
                        {
                            row_action = Some("down");
                        }
                        ui.separator();
                        ui.label("Slew gap");
                        ui.add(
                            egui::DragValue::new(&mut self.skd_slew_gap_sec)
                                .speed(5)
                                .range(0..=86400)
                                .suffix(" s"),
                        );
                        if ui
                            .add_enabled(
                                selected_index.is_some_and(|index| index > 0),
                                egui::Button::new("Start After Previous"),
                            )
                            .on_hover_text(
                                "Set this row to the previous row's end time plus the slew gap.",
                            )
                            .clicked()
                        {
                            row_action = Some("after_previous");
                        }
                        if ui
                            .add_enabled(
                                selected_count > 0,
                                egui::Button::new("Delete Selected"),
                            )
                            .clicked()
                        {
                            row_action = Some("delete");
                        }
                    });
                    if let Some(action) = row_action {
                        let result = match action {
                            "duplicate" => self.duplicate_selected_skd_row(),
                            "up" => self.move_selected_skd_row(-1),
                            "down" => self.move_selected_skd_row(1),
                            "after_previous" => self.set_selected_start_after_previous(),
                            "delete" => self.delete_selected_skd_rows(),
                            _ => Ok(()),
                        };
                        match result {
                            Ok(()) => self.error_msg = None,
                            Err(error) => self.error_msg = Some(error),
                        }
                    }

                    ui.horizontal_wrapped(|ui| {
                        ui.strong("Auto schedule");
                        ui.label("Extra margin");
                        ui.add(
                            egui::DragValue::new(&mut self.skd_auto_slew_margin_sec)
                                .speed(1)
                                .range(0..=3600)
                                .suffix(" s"),
                        );
                        if ui
                            .add_enabled(
                                self.skd_rows.len() >= 2,
                                egui::Button::new("Recalculate Selected Row Onward"),
                            )
                            .on_hover_text(
                                "Use the actual required slew time of both selected antennas.",
                            )
                            .clicked()
                        {
                            match self.auto_schedule_from_selected() {
                                Ok(()) => self.error_msg = Some(
                                    "Recalculated scan times using actual antenna slew."
                                        .to_string(),
                                ),
                                Err(error) => self.error_msg = Some(error),
                            }
                        }
                    });

                    egui::CollapsingHeader::new("Observation Block / Bulk Edit")
                        .default_open(false)
                        .show(ui, |ui| {
                            let mut advanced_action = None;
                            ui.horizontal_wrapped(|ui| {
                                ui.strong("Repeat selected block");
                                if ui
                                    .add_enabled(
                                        !self.selected_skd_rows.is_empty(),
                                        egui::Button::new("Copy Block"),
                                    )
                                    .clicked()
                                {
                                    advanced_action = Some("copy");
                                }
                                ui.label(format!(
                                    "Clipboard: {} rows",
                                    self.skd_block_clipboard.len()
                                ));
                                if ui
                                    .add_enabled(
                                        !self.skd_block_clipboard.is_empty()
                                            && self.selected_skd_row.is_some(),
                                        egui::Button::new("Paste Before"),
                                    )
                                    .clicked()
                                {
                                    advanced_action = Some("paste_before");
                                }
                                if ui
                                    .add_enabled(
                                        !self.skd_block_clipboard.is_empty()
                                            && self.selected_skd_row.is_some(),
                                        egui::Button::new("Paste After"),
                                    )
                                    .clicked()
                                {
                                    advanced_action = Some("paste_after");
                                }
                                ui.label("Copies");
                                ui.add(
                                    egui::DragValue::new(&mut self.skd_block_repeat_count)
                                        .range(1..=1000),
                                );
                                ui.label("Gap");
                                ui.add(
                                    egui::DragValue::new(&mut self.skd_block_gap_sec)
                                        .speed(5)
                                        .range(0..=86400)
                                        .suffix(" s"),
                                );
                                if ui
                                    .add_enabled(
                                        !self.selected_skd_rows.is_empty(),
                                        egui::Button::new("Repeat Block"),
                                    )
                                    .clicked()
                                {
                                    advanced_action = Some("repeat");
                                }
                            });
                            ui.separator();
                            ui.horizontal_wrapped(|ui| {
                                ui.strong("Bulk edit selected rows");
                                if !self.sources.is_empty() {
                                    self.skd_bulk_source_index =
                                        self.skd_bulk_source_index.min(self.sources.len() - 1);
                                    egui::ComboBox::from_id_salt("skd_bulk_source")
                                        .selected_text(
                                            &self.sources[self.skd_bulk_source_index].0.name,
                                        )
                                        .show_ui(ui, |ui| {
                                            for (index, (source, _)) in
                                                self.sources.iter().enumerate()
                                            {
                                                ui.selectable_value(
                                                    &mut self.skd_bulk_source_index,
                                                    index,
                                                    &source.name,
                                                );
                                            }
                                        });
                                    if ui.button("Apply Source").clicked() {
                                        advanced_action = Some("source");
                                    }
                                }
                                ui.label("Duration");
                                ui.add(
                                    egui::DragValue::new(&mut self.skd_bulk_duration_sec)
                                        .speed(10)
                                        .range(1..=86400)
                                        .suffix(" s"),
                                );
                                if ui.button("Apply Duration").clicked() {
                                    advanced_action = Some("duration");
                                }
                                ui.label("Shift");
                                ui.add(
                                    egui::DragValue::new(&mut self.skd_bulk_shift_sec)
                                        .speed(10)
                                        .range(-86400..=86400)
                                        .suffix(" s"),
                                );
                                if ui.button("Shift Times").clicked() {
                                    advanced_action = Some("shift");
                                }
                            });
                            ui.horizontal_wrapped(|ui| {
                                ui.label("Offsets (deg)  AZ");
                                ui.add(egui::DragValue::new(
                                    &mut self.skd_bulk_az_offset_deg,
                                ));
                                ui.label("EL");
                                ui.add(egui::DragValue::new(
                                    &mut self.skd_bulk_el_offset_deg,
                                ));
                                ui.label("RA");
                                ui.add(egui::DragValue::new(
                                    &mut self.skd_bulk_ra_offset_deg,
                                ));
                                ui.label("Dec");
                                ui.add(egui::DragValue::new(
                                    &mut self.skd_bulk_dec_offset_deg,
                                ));
                                if ui.button("Apply Offsets").clicked() {
                                    advanced_action = Some("offsets");
                                }
                            });
                            if let Some(action) = advanced_action {
                                let result = match action {
                                    "copy" => self.copy_selected_skd_block(),
                                    "paste_before" => self
                                        .selected_skd_row
                                        .ok_or_else(|| "Select an insertion row first.".to_string())
                                        .and_then(|index| self.paste_skd_block(index)),
                                    "paste_after" => self
                                        .selected_skd_row
                                        .ok_or_else(|| "Select an insertion row first.".to_string())
                                        .and_then(|index| self.paste_skd_block(index + 1)),
                                    "repeat" => self.repeat_selected_skd_block(),
                                    "source" => self.apply_bulk_source(),
                                    "duration" => self.apply_bulk_duration(),
                                    "shift" => self.apply_bulk_time_shift(),
                                    "offsets" => self.apply_bulk_offsets(),
                                    _ => Ok(()),
                                };
                                match result {
                                    Ok(()) => self.error_msg = None,
                                    Err(error) => self.error_msg = Some(error),
                                }
                            }
                        });

                    ui.separator();
                    self.rebuild_skd_status_cache();
                    let problem_indices = self.skd_problem_indices();
                    ui.horizontal_wrapped(|ui| {
                        ui.checkbox(&mut self.skd_show_problems_only, "Problems only");
                        ui.label(format!("{} issues", problem_indices.len()));
                        if ui
                            .add_enabled(
                                !problem_indices.is_empty(),
                                egui::Button::new("Previous Issue"),
                            )
                            .clicked()
                        {
                            let current = self.selected_skd_row.unwrap_or(self.skd_rows.len());
                            let target = problem_indices
                                .iter()
                                .rev()
                                .copied()
                                .find(|index| *index < current)
                                .or_else(|| problem_indices.last().copied());
                            if let Some(index) = target {
                                self.select_only_skd_row(index);
                                self.skd_scroll_to_row = Some(index);
                            }
                        }
                        if ui
                            .add_enabled(
                                !problem_indices.is_empty(),
                                egui::Button::new("Next Issue"),
                            )
                            .clicked()
                        {
                            let current = self.selected_skd_row.unwrap_or(usize::MAX);
                            let target = problem_indices
                                .iter()
                                .copied()
                                .find(|index| current == usize::MAX || *index > current)
                                .or_else(|| problem_indices.first().copied());
                            if let Some(index) = target {
                                self.select_only_skd_row(index);
                                self.skd_scroll_to_row = Some(index);
                            }
                        }
                        ui.separator();
                        ui.checkbox(&mut self.skd_show_timeline, "Timeline");
                        ui.small("Click row numbers; Ctrl-click and Shift-click select blocks.");
                    });
                    if self.skd_show_timeline {
                        ui.small(
                            "Timeline: upper lane = Observation; lower lane = Slew (orange) / Idle (gray).",
                        );
                        self.show_skd_timeline(ui);
                    }

                    let mut remove_idx = None;
                    let mut table_changed = false;
                    let rows_before_table_edit = self.skd_rows.clone();
                    egui::ScrollArea::both()
                        .id_salt("skd_schedule_scroll")
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.set_min_width(SKD_TABLE_MIN_WIDTH);
                            ui.horizontal(|ui| {
                                show_table_text_cell(ui, SKD_COL_NUM, 18.0, "#");
                                show_table_text_cell(ui, SKD_COL_SOURCE, 18.0, "Source");
                                show_table_text_cell(ui, SKD_COL_DATE, 18.0, "Date");
                                show_table_text_cell(ui, SKD_COL_TIME, 18.0, "Start UT");
                                show_table_text_cell(ui, SKD_COL_DURATION, 18.0, "Duration");
                                show_table_text_cell(ui, SKD_COL_AZEL, 18.0, "Start Az/El");
                                show_table_text_cell(ui, SKD_COL_AZEL, 18.0, "End Az/El");
                                show_table_text_cell(ui, SKD_COL_ANTENNA, 18.0, "Antenna 1");
                                show_table_text_cell(ui, SKD_COL_ANTENNA, 18.0, "Antenna 2");
                                show_table_text_cell(ui, SKD_COL_DELETE, 18.0, "");
                            });
                            ui.horizontal(|ui| {
                                show_table_text_cell(ui, SKD_COL_NUM, 16.0, "");
                                show_table_text_cell(ui, SKD_COL_SOURCE, 16.0, "");
                                show_table_text_cell(ui, SKD_COL_DATE, 16.0, "YYYY-MM-DD");
                                show_table_text_cell(ui, SKD_COL_TIME, 16.0, "hh:mm:ss");
                                show_table_text_cell(ui, SKD_COL_DURATION, 16.0, "s");
                                show_table_text_cell(ui, SKD_COL_AZEL, 16.0, "deg");
                                show_table_text_cell(ui, SKD_COL_AZEL, 16.0, "deg");
                                show_table_text_cell(ui, SKD_COL_ANTENNA, 16.0, "limits / slew");
                                show_table_text_cell(ui, SKD_COL_ANTENNA, 16.0, "limits / slew");
                                show_table_text_cell(ui, SKD_COL_DELETE, 16.0, "");
                            });
                            ui.separator();

                            for i in 0..self.skd_rows.len() {
                                let status =
                                    self.skd_status_cache
                                        .get(i)
                                        .cloned()
                                        .unwrap_or(SkdRowStatus {
                                            start_geometry: "...".to_string(),
                                            end_geometry: "...".to_string(),
                                            motion_1: "...".to_string(),
                                            motion_2: "...".to_string(),
                                        });
                                if self.skd_show_problems_only
                                    && !skd_status_has_problem(&status)
                                {
                                    continue;
                                }
                                let start_geometry = status.start_geometry;
                                let end_geometry = status.end_geometry;
                                let motion_check_1 = status.motion_1;
                                let motion_check_2 = status.motion_2;
                                ui.horizontal(|ui| {
                                    let row_number_response = ui
                                        .add_sized(
                                            [SKD_COL_NUM, 20.0],
                                            egui::Button::new((i + 1).to_string())
                                                .selected(self.selected_skd_rows.contains(&i)),
                                        )
                                        .on_hover_text(
                                            "Select row; Ctrl-click toggles and Shift-click selects a range.",
                                        );
                                    if self.skd_scroll_to_row == Some(i) {
                                        row_number_response
                                            .scroll_to_me(Some(egui::Align::Center));
                                        self.skd_scroll_to_row = None;
                                    }
                                    if row_number_response.clicked() {
                                        let modifiers = ui.input(|input| input.modifiers);
                                        self.select_skd_row(i, modifiers);
                                    }
                                    let selected_source = self.skd_rows[i].source_name.clone();
                                    egui::ComboBox::from_id_salt(format!("skd_source_{}", i))
                                        .width(SKD_COL_SOURCE)
                                        .selected_text(source_table_text(&selected_source))
                                        .show_ui(ui, |ui| {
                                            for (source, _) in &self.sources {
                                                if ui
                                                    .selectable_value(
                                                        &mut self.skd_rows[i].source_name,
                                                        source.name.clone(),
                                                        &source.name,
                                                    )
                                                    .changed()
                                                {
                                                    table_changed = true;
                                                }
                                            }
                                        });
                                    ui.add_sized(
                                        [SKD_COL_DATE, 20.0],
                                        egui::Label::new(
                                            self.skd_rows[i]
                                                .start_date
                                                .format("%Y-%m-%d")
                                                .to_string(),
                                        ),
                                    );
                                    if ui
                                        .add_sized(
                                            [SKD_COL_TIME, 20.0],
                                            egui::TextEdit::singleline(
                                                &mut self.skd_rows[i].start_time,
                                            ),
                                        )
                                        .changed()
                                    {
                                        table_changed = true;
                                    }
                                    if ui
                                        .add_sized(
                                            [SKD_COL_DURATION, 20.0],
                                            egui::DragValue::new(
                                                &mut self.skd_rows[i].duration_sec,
                                            )
                                            .speed(10)
                                            .range(1..=86400),
                                        )
                                        .changed()
                                    {
                                        table_changed = true;
                                    }
                                    ui.add_sized(
                                        [SKD_COL_AZEL, 20.0],
                                        egui::Label::new(start_geometry),
                                    )
                                    .on_hover_text("Start AZ/EL");
                                    ui.add_sized(
                                        [SKD_COL_AZEL, 20.0],
                                        egui::Label::new(end_geometry),
                                    )
                                    .on_hover_text("End AZ/EL");
                                    for motion_check in [motion_check_1, motion_check_2] {
                                        show_motion_status_cell(ui, &motion_check).on_hover_text(
                                            "Checks start/end AZ/EL limits and required slew time.",
                                        );
                                    }
                                    if ui
                                        .add_sized([SKD_COL_DELETE, 20.0], egui::Button::new("Del"))
                                        .clicked()
                                    {
                                        remove_idx = Some(i);
                                    }
                                });
                            }
                        });
                    if let Some(i) = remove_idx {
                        self.delete_skd_row(i);
                    } else if table_changed {
                        self.record_skd_undo_snapshot(rows_before_table_edit);
                        self.mark_skd_status_dirty();
                    }
                },
            );
        });

        if let Some(err) = &self.error_msg {
            ui.add_space(8.0);
            ui.colored_label(egui::Color32::RED, err);
        }
    }

    fn ui_uptime_plotters_tab(&mut self, ui: &mut egui::Ui) {
        let station_pos = self.station_position();
        let selected_date = self.selected_date;
        let az_pointer_formatter = move |x: f64, y: f64| {
            let ut_text = format!("UT: {}", format_hour_hms(x));
            let lst_text = station_pos
                .and_then(|pos| {
                    utc_datetime_from_hour(selected_date, x)
                        .map(|dt| utils::utc_to_lst_hours(pos, dt))
                })
                .map(|lst| format!("LST: {}", format_hour_hms(lst)))
                .unwrap_or_else(|| "LST: N/A".to_string());
            format!("{}\n{}\nAz: {:.1}°", ut_text, lst_text, y)
        };
        let el_pointer_formatter = move |x: f64, y: f64| {
            let ut_text = format!("UT: {}", format_hour_hms(x));
            let lst_text = station_pos
                .and_then(|pos| {
                    utc_datetime_from_hour(selected_date, x)
                        .map(|dt| utils::utc_to_lst_hours(pos, dt))
                })
                .map(|lst| format!("LST: {}", format_hour_hms(lst)))
                .unwrap_or_else(|| "LST: N/A".to_string());
            format!("{}\n{}\nEl: {:.1}°", ut_text, lst_text, y)
        };

        let plot_az = Plot::new("az_plot")
            .width(ui.available_width())
            .height(ui.available_height() / 2.0)
            .y_axis_label("Azimuth (deg)")
            .y_axis_min_width(PLOT_Y_AXIS_MIN_WIDTH)
            .include_x(0.0)
            .include_x(24.0)
            .include_y(-5.0)
            .include_y(365.0)
            .allow_drag(false)
            .allow_zoom(false)
            .allow_scroll(false)
            .x_axis_label("") // Re-added
            .x_axis_formatter(|_, _| "".to_string()) // Re-added
            .x_grid_spacer(|_input| {
                [
                    0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0, 13.0, 14.0,
                    15.0, 16.0, 17.0, 18.0, 19.0, 20.0, 21.0, 22.0, 23.0, 24.0,
                ]
                .into_iter()
                .map(|v| GridMark {
                    value: v,
                    step_size: 3.0,
                })
                .collect::<Vec<_>>()
            })
            .y_grid_spacer(|_input| {
                [
                    0.0, 30.0, 60.0, 90.0, 120.0, 150.0, 180.0, 210.0, 240.0, 270.0, 300.0, 330.0,
                    360.0,
                ]
                .into_iter()
                .map(|v| GridMark {
                    value: v,
                    step_size: 30.0,
                })
                .collect::<Vec<_>>()
            })
            .y_axis_formatter(|m, _| format!("{:.0}", m.value))
            .show_y(true)
            .coordinates_formatter(
                Corner::LeftTop,
                egui_plot::CoordinatesFormatter::new(move |plot_point, _plot_bounds| {
                    az_pointer_formatter(plot_point.x, plot_point.y)
                }),
            )
            .legend(Legend::default());

        let plot_el = Plot::new("el_plot")
            .width(ui.available_width())
            .height(ui.available_height() / 2.0)
            .x_axis_label("Time (UT)")
            .y_axis_label("Elevation (deg)")
            .y_axis_min_width(PLOT_Y_AXIS_MIN_WIDTH)
            .include_x(0.0)
            .include_x(24.0)
            .include_y(0.0)
            .include_y(91.0)
            .allow_drag(false)
            .allow_zoom(false)
            .allow_scroll(false)
            .x_grid_spacer(|_input| {
                [
                    0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0, 13.0, 14.0,
                    15.0, 16.0, 17.0, 18.0, 19.0, 20.0, 21.0, 22.0, 23.0, 24.0,
                ]
                .into_iter()
                .map(|v| GridMark {
                    value: v,
                    step_size: 3.0,
                })
                .collect::<Vec<_>>()
            })
            .y_grid_spacer(|_input| {
                [0.0, 10.0, 20.0, 30.0, 40.0, 50.0, 60.0, 70.0, 80.0, 90.0]
                    .into_iter()
                    .map(|v| GridMark {
                        value: v,
                        step_size: 10.0,
                    })
                    .collect::<Vec<_>>()
            })
            .x_axis_formatter(|m, _| format!("{:.0}", m.value as u32))
            .y_axis_formatter(|m, _| format!("{:.0}", m.value))
            .show_x(true)
            .coordinates_formatter(
                Corner::LeftTop,
                egui_plot::CoordinatesFormatter::new(move |plot_point, _plot_bounds| {
                    el_pointer_formatter(plot_point.x, plot_point.y)
                }),
            )
            .legend(Legend::default());

        let az_response = plot_az.show(ui, |plot_ui| {
            plot_ui.set_plot_bounds(egui_plot::PlotBounds::from_min_max(
                [0.0, -5.0],
                [24.7, 365.0],
            ));
            for (name, az_points, _) in &self.plot_data {
                let mut line = Line::new(name.clone(), PlotPoints::from(az_points.clone()));
                if is_sun_track(name) {
                    line = line.color(egui::Color32::YELLOW);
                }
                plot_ui.line(line);
            }
        });

        ui.add_space(-10.0);

        let el_response = plot_el.show(ui, |plot_ui| {
            plot_ui.set_plot_bounds(egui_plot::PlotBounds::from_min_max(
                [0.0, 0.0],
                [24.7, 91.0],
            ));
            for (name, _, el_points) in &self.plot_data {
                let mut line = Line::new(name.clone(), PlotPoints::from(el_points.clone()));
                if is_sun_track(name) {
                    line = line.color(egui::Color32::YELLOW);
                }
                plot_ui.line(line);
            }
        });

        self.uptime_plot_rect = Some(az_response.response.rect.union(el_response.response.rect));
    }

    fn ui_parameters_tab(&mut self, ui: &mut egui::Ui) {
        ui.heading("Parameters");
        ui.add_space(10.0);

        let available = ui.available_size();
        let panel_width = (available.x - 28.0).max(480.0);
        ui.horizontal(|ui| {
            ui.allocate_ui_with_layout(
                egui::vec2(available.x, available.y),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("parameters_scroll")
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.set_min_width(panel_width);
                            ui.set_max_width(panel_width);

                            let settings_column_width = ((panel_width - 40.0) / 3.0).max(240.0);
                            let settings_frame = egui::Frame::group(ui.style());
                            ui.columns(3, |columns| {
                                // --- Station Settings ---
                                settings_frame.show(&mut columns[0], |ui| {
                                    ui.set_min_width(settings_column_width);
                                    ui.set_max_width(settings_column_width);
                                    ui.heading("📡 Station Settings");
                                    ui.add_space(5.0);

                                    ui.label("Stations:");
                                    if self.stations.is_empty() {
                                        ui.label("Load stations from station.txt");
                                    } else {
                                        if self.selected_stations.len() != self.stations.len() {
                                            self.selected_stations
                                                .resize(self.stations.len(), false);
                                            let fallback =
                                                self.selected_station.min(self.stations.len() - 1);
                                            if !self
                                                .selected_stations
                                                .iter()
                                                .any(|selected| *selected)
                                            {
                                                self.selected_stations[fallback] = true;
                                            }
                                        }

                                        let mut changed = false;
                                        egui::Grid::new("station_checkbox_grid")
                                            .num_columns(3)
                                            .spacing([20.0, 4.0])
                                            .show(ui, |ui| {
                                                for i in 0..self.stations.len() {
                                                    let station_name =
                                                        self.stations[i].name.clone();
                                                    if ui
                                                        .checkbox(
                                                            &mut self.selected_stations[i],
                                                            station_name,
                                                        )
                                                        .changed()
                                                    {
                                                        changed = true;
                                                        self.selected_station = i;
                                                    }

                                                    if (i + 1) % 3 == 0 {
                                                        ui.end_row();
                                                    }
                                                }
                                            });

                                        if changed {
                                            self.clear_plot_data();
                                        }
                                    }

                                    ui.separator();
                                    ui.label("Station File:");
                                    ui.horizontal_wrapped(|ui| {
                                        ui.add_sized(
                                            [(settings_column_width - 20.0).max(180.0), 24.0],
                                            egui::TextEdit::singleline(&mut self.station_file_path),
                                        );
                                        if ui.button("Load").clicked() {
                                            match pick_file_dialog("Select station.txt") {
                                                Ok(Some(path)) => {
                                                    self.station_file_path =
                                                        path.to_string_lossy().to_string();
                                                    match self.load_stations() {
                                                        Ok(_) => self.error_msg = None,
                                                        Err(e) => self.error_msg = Some(e),
                                                    }
                                                }
                                                Ok(None) => {}
                                                Err(e) => self.error_msg = Some(e),
                                            }
                                        }
                                        if ui.button("Reload").clicked() {
                                            match self.load_stations() {
                                                Ok(_) => self.error_msg = None,
                                                Err(e) => self.error_msg = Some(e),
                                            }
                                        }
                                        if ui.button("Open").clicked() {
                                            match utils::open_file_in_external_editor(
                                                &self.station_file_path,
                                            ) {
                                                Ok(_) => self.error_msg = None,
                                                Err(e) => self.error_msg = Some(e),
                                            }
                                        }
                                    });
                                });

                                // --- Observation Settings ---
                                settings_frame.show(&mut columns[1], |ui| {
                                    ui.set_min_width(settings_column_width);
                                    ui.set_max_width(settings_column_width);
                                    ui.heading("Observation Settings");
                                    ui.add_space(5.0);

                                    ui.horizontal_wrapped(|ui| {
                                        ui.label("Observation Date:");
                                        if ui
                                            .button(
                                                self.selected_date.format("%Y-%m-%d").to_string(),
                                            )
                                            .clicked()
                                        {
                                            self.show_calendar = !self.show_calendar;
                                        }
                                    });

                                    ui.horizontal_wrapped(|ui| {
                                        ui.label("LST at 00:00 UT:");
                                        if let Some(station_pos) = self.station_position() {
                                            if let Some(lst_hours) =
                                                self.lst_from_ut_hour(station_pos, 0.0)
                                            {
                                                ui.label(format_hour_hms(lst_hours));
                                            } else {
                                                ui.label("N/A");
                                            }
                                        } else {
                                            ui.label("N/A");
                                        }
                                    });
                                });

                                // --- File Formats ---
                                settings_frame.show(&mut columns[2], |ui| {
                                    ui.set_min_width(settings_column_width);
                                    ui.set_max_width(settings_column_width);
                                    ui.heading("📄 File Format Information");
                                    ui.add_space(5.0);
                                    ui.add(
                                        egui::Label::new(
                                            "station.txt format (ECEF): NAME X_POS Y_POS Z_POS",
                                        )
                                        .wrap(),
                                    );
                                    ui.add(
                                        egui::Label::new(
                                            "e.g. YAMAGU32 -3502544.587 3950966.235 3566381.192",
                                        )
                                        .wrap(),
                                    );
                                    ui.separator();
                                    ui.add(
                                        egui::Label::new(
                                            "source.txt format: NAME RA_H RA_M RA_S DEC_D DEC_M DEC_S",
                                        )
                                        .wrap(),
                                    );
                                    ui.add(
                                        egui::Label::new("e.g. 3C273 12 29 06.7 +02 03 08.6")
                                            .wrap(),
                                    );
                                });
                            });

                            ui.add_space(10.0);

                            // --- Source Settings ---
                            egui::Frame::group(ui.style()).show(ui, |ui| {
                                ui.set_min_width(panel_width);
                                ui.set_max_width(panel_width);
                                ui.heading("🔭 Source Settings");
                                ui.add_space(5.0);

                                ui.label("Source List File:");
                                ui.horizontal_wrapped(|ui| {
                                    ui.add_sized(
                                        [(panel_width - 220.0).max(180.0), 24.0],
                                        egui::TextEdit::singleline(&mut self.source_file_path),
                                    );
                                    if ui.button("Load").clicked() {
                                        match pick_file_dialog("Select source.txt") {
                                            Ok(Some(path)) => {
                                                self.source_file_path =
                                                    path.to_string_lossy().to_string();
                                                match self.load_sources() {
                                                    Ok(_) => self.error_msg = None,
                                                    Err(e) => self.error_msg = Some(e),
                                                }
                                            }
                                            Ok(None) => {}
                                            Err(e) => self.error_msg = Some(e),
                                        }
                                    }
                                    if ui.button("Reload").clicked() {
                                        match self.load_sources() {
                                            Ok(_) => self.error_msg = None,
                                            Err(e) => self.error_msg = Some(e),
                                        }
                                    }
                                    if ui.button("Open").clicked() {
                                        match utils::open_file_in_external_editor(
                                            &self.source_file_path,
                                        ) {
                                            Ok(_) => self.error_msg = None,
                                            Err(e) => self.error_msg = Some(e),
                                        }
                                    }
                                });

                                ui.horizontal_wrapped(|ui| {
                                    ui.label("Search Filter:");
                                    ui.add_sized(
                                        [(panel_width - 230.0).max(180.0), 24.0],
                                        egui::TextEdit::singleline(&mut self.search_query),
                                    );
                                    if !self.search_query.is_empty()
                                        && ui.button("Clear Search").clicked()
                                    {
                                        self.search_query.clear();
                                    }
                                });

                                ui.separator();
                                ui.label("Select Sources to Plot:");
                                ui.horizontal_wrapped(|ui| {
                                    if ui.button("Plot Selected").clicked() {
                                        self.calculate_plots();
                                    }
                                    if ui.button("output").clicked() {
                                        match self.start_output_capture(ui.ctx()) {
                                            Ok(_) => {
                                                self.error_msg =
                                                    Some("Output started...".to_string())
                                            }
                                            Err(e) => self.error_msg = Some(e),
                                        }
                                    }
                                    if ui.button("Select Filtered").clicked() {
                                        let query = self.search_query.to_lowercase();
                                        for (source, selected) in &mut self.sources {
                                            if query.is_empty()
                                                || source.name.to_lowercase().contains(&query)
                                            {
                                                *selected = true;
                                            }
                                        }
                                    }
                                    if ui.button("Clear Filtered").clicked() {
                                        let query = self.search_query.to_lowercase();
                                        for (source, selected) in &mut self.sources {
                                            if query.is_empty()
                                                || source.name.to_lowercase().contains(&query)
                                            {
                                                *selected = false;
                                            }
                                        }
                                    }
                                    if ui.button("Clear All").clicked() {
                                        for (_, selected) in &mut self.sources {
                                            *selected = false;
                                        }
                                    }
                                    let selected_count = self
                                        .sources
                                        .iter()
                                        .filter(|(_, selected)| *selected)
                                        .count();
                                    ui.label(format!("{} selected", selected_count));
                                });

                                if self.sources.is_empty() {
                                    ui.label("(No sources loaded)");
                                } else {
                                    let source_columns =
                                        ((panel_width / 150.0).floor() as usize).clamp(4, 12);
                                    egui::ScrollArea::vertical()
                                        .id_salt("parameters_source_list")
                                        .max_height(460.0)
                                        .auto_shrink([false, false])
                                        .show(ui, |ui| {
                                            egui::Grid::new("source_grid")
                                                .num_columns(source_columns)
                                                .spacing([20.0, 4.0])
                                                .show(ui, |ui| {
                                                    let mut displayed_count = 0;
                                                    let query = self.search_query.to_lowercase();
                                                    for (source, selected) in
                                                        self.sources.iter_mut()
                                                    {
                                                        if query.is_empty()
                                                            || source
                                                                .name
                                                                .to_lowercase()
                                                                .contains(&query)
                                                        {
                                                            ui.checkbox(selected, &source.name);
                                                            displayed_count += 1;
                                                            if displayed_count % source_columns == 0
                                                            {
                                                                ui.end_row();
                                                            }
                                                        }
                                                    }
                                                });
                                        });
                                }
                            });

                            if let Some(err) = &self.error_msg {
                                ui.add_space(10.0);
                                ui.colored_label(egui::Color32::RED, err);
                            }
                        });
                },
            );
        });
    }

    fn ui_polar_plot_tab(&mut self, ui: &mut egui::Ui) {
        //ui.heading("Polar Plot");

        let plot = Plot::new("polar_plot")
            .width(ui.available_width()) // Added
            .height(ui.available_height()) // Added
            .data_aspect(1.0) // Ensure circular aspect ratio
            .view_aspect(1.0) // Ensure circular aspect ratio
            .include_x(-1.0)
            .include_x(1.0) // Cartesian coordinates for polar plot
            .include_y(-1.0)
            .include_y(1.0) // Cartesian coordinates for polar plot
            .center_x_axis(true)
            .center_y_axis(true)
            .show_x(false) // Hide Cartesian x-axis
            .show_y(false) // Hide Cartesian y-axis
            .x_grid_spacer(|_input| vec![]) // Disable x-grid
            .y_grid_spacer(|_input| vec![]) // Disable y-grid
            .legend(Legend::default());

        let polar_response = plot.show(ui, |plot_ui| {
            // Draw circles for elevation levels (e.g., 0, 30, 60, 90)
            // 90 deg el is center (radius 0), 0 deg el is outer edge (radius 1)
            // So, radius = (90 - el) / 90
            for el_level in [0.0, 15.0, 30.0, 45.0, 60.0, 75.0, 90.0] {
                let radius = (90.0 - el_level) / 90.0;
                if radius >= 0.0 {
                    // Ensure radius is non-negative
                    let num_segments = 100;
                    let mut circle_points = Vec::new();
                    for i in 0..=num_segments {
                        let angle = i as f64 * 2.0 * std::f64::consts::PI / num_segments as f64;
                        let x = radius * angle.cos();
                        let y = radius * angle.sin();
                        circle_points.push([x, y]);
                    }
                    plot_ui.line(
                        Line::new("", PlotPoints::from(circle_points))
                            .stroke(egui::Stroke::new(2.0, egui::Color32::DARK_GRAY)),
                    );

                    // Add elevation labels
                    if el_level != 90.0 {
                        // Don't label the center point
                        let label_text = format!("{:.0}°", el_level);
                        // Position the label slightly inside the circle, at 0 azimuth (North)
                        let label_x = radius * (72.0f64).to_radians().cos();
                        let label_y = radius * (72.0f64).to_radians().sin();
                        plot_ui.text(
                            egui_plot::Text::new(
                                "",
                                egui_plot::PlotPoint::new(label_x, label_y),
                                label_text,
                            )
                            .color(egui::Color32::DARK_GRAY),
                        );
                    }
                }
            }

            // Draw radial lines for azimuth (e.g., 0, 90, 180, 270)
            for az_level in [0.0, 45.0, 90.0, 135.0, 180.0, 225.0, 270.0, 315.0] {
                let angle_rad = (90.0f64 - az_level).to_radians(); // Adjust for egui_plot's 0 deg at positive x-axis, clockwise
                let x = 1.0 * angle_rad.cos();
                let y = 1.0 * angle_rad.sin();
                plot_ui.line(
                    Line::new("", PlotPoints::from(vec![[0.0, 0.0], [x, y]]))
                        .stroke(egui::Stroke::new(2.0, egui::Color32::DARK_GRAY)),
                );

                // Add azimuth labels
                let label_text = format!("{:.0}°", az_level);
                plot_ui.text(
                    egui_plot::Text::new(
                        "",
                        egui_plot::PlotPoint::new(x * 1.1, y * 1.1),
                        label_text,
                    )
                    .color(egui::Color32::DARK_GRAY),
                );
            }

            for (name, polar_points, hour_marker_points, hour_labels) in &self.polar_plot_data {
                let sun_track = is_sun_track(name);
                if !polar_points.is_empty() {
                    let mut points =
                        Points::new(name.clone(), PlotPoints::from(polar_points.clone()));
                    if sun_track {
                        points = points.color(egui::Color32::YELLOW);
                    }
                    plot_ui.points(points);
                }
                if !hour_marker_points.is_empty() {
                    let mut markers =
                        Points::new("", PlotPoints::from(hour_marker_points.clone())).radius(3.5);
                    if sun_track {
                        markers = markers.color(egui::Color32::YELLOW);
                    }
                    plot_ui.points(markers);
                    for (label_x, label_y, label_text) in hour_labels {
                        plot_ui.text(
                            egui_plot::Text::new(
                                "",
                                egui_plot::PlotPoint::new(*label_x, *label_y),
                                label_text.clone(),
                            )
                            .color(if sun_track {
                                egui::Color32::YELLOW
                            } else {
                                egui::Color32::LIGHT_GRAY
                            }),
                        );
                    }
                }
            }

            let fixed_angle_rad = (90.0 - ANTENNA_FIXED_AZ_DEG).to_radians();
            let fixed_radius = (90.0 - ANTENNA_FIXED_EL_DEG) / 90.0;
            let fixed_position = vec![[
                fixed_radius * fixed_angle_rad.cos(),
                fixed_radius * fixed_angle_rad.sin(),
            ]];
            plot_ui.points(
                Points::new(
                    format!(
                        "Antenna fixed position (AZ {:.0}°, EL {:.0}°)",
                        ANTENNA_FIXED_AZ_DEG, ANTENNA_FIXED_EL_DEG
                    ),
                    PlotPoints::from(fixed_position),
                )
                .radius(7.0)
                .color(egui::Color32::RED),
            );
        });
        self.polar_plot_rect = Some(polar_response.response.rect);
    }

    fn ui_lst_plot_tab(&mut self, ui: &mut egui::Ui) {
        if self.stations.is_empty() {
            ui.label("No station selected.");
            self.lst_plot_rect = None;
            return;
        }

        let lst_plot_data = &self.lst_plot_data;
        let az_pointer_formatter =
            |x: f64, y: f64| format!("LST: {}\nAz: {:.1}°", format_hour_hms(x), y);
        let el_pointer_formatter =
            |x: f64, y: f64| format!("LST: {}\nEl: {:.1}°", format_hour_hms(x), y);

        let plot_az = Plot::new("lst_az_plot")
            .width(ui.available_width())
            .height(ui.available_height() / 2.0)
            .y_axis_label("Azimuth (deg)")
            .y_axis_min_width(PLOT_Y_AXIS_MIN_WIDTH)
            .include_x(0.0)
            .include_x(24.0)
            .include_y(-5.0)
            .include_y(365.0)
            .allow_drag(false)
            .allow_zoom(false)
            .allow_scroll(false)
            .x_axis_label("")
            .x_axis_formatter(|_, _| "".to_string())
            .x_grid_spacer(|_input| {
                [
                    0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0, 13.0, 14.0,
                    15.0, 16.0, 17.0, 18.0, 19.0, 20.0, 21.0, 22.0, 23.0, 24.0,
                ]
                .into_iter()
                .map(|v| GridMark {
                    value: v,
                    step_size: 3.0,
                })
                .collect::<Vec<_>>()
            })
            .y_grid_spacer(|_input| {
                [
                    0.0, 30.0, 60.0, 90.0, 120.0, 150.0, 180.0, 210.0, 240.0, 270.0, 300.0, 330.0,
                    360.0,
                ]
                .into_iter()
                .map(|v| GridMark {
                    value: v,
                    step_size: 30.0,
                })
                .collect::<Vec<_>>()
            })
            .y_axis_formatter(|m, _| format!("{:.0}", m.value))
            .show_y(true)
            .coordinates_formatter(
                Corner::LeftTop,
                egui_plot::CoordinatesFormatter::new(move |plot_point, _plot_bounds| {
                    az_pointer_formatter(plot_point.x, plot_point.y)
                }),
            )
            .legend(Legend::default());

        let plot_el = Plot::new("lst_el_plot")
            .width(ui.available_width())
            .height(ui.available_height() / 2.0)
            .x_axis_label("Time (LST)")
            .y_axis_label("Elevation (deg)")
            .y_axis_min_width(PLOT_Y_AXIS_MIN_WIDTH)
            .include_x(0.0)
            .include_x(24.0)
            .include_y(0.0)
            .include_y(91.0)
            .allow_drag(false)
            .allow_zoom(false)
            .allow_scroll(false)
            .x_grid_spacer(|_input| {
                [
                    0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0, 13.0, 14.0,
                    15.0, 16.0, 17.0, 18.0, 19.0, 20.0, 21.0, 22.0, 23.0, 24.0,
                ]
                .into_iter()
                .map(|v| GridMark {
                    value: v,
                    step_size: 3.0,
                })
                .collect::<Vec<_>>()
            })
            .y_grid_spacer(|_input| {
                [0.0, 10.0, 20.0, 30.0, 40.0, 50.0, 60.0, 70.0, 80.0, 90.0]
                    .into_iter()
                    .map(|v| GridMark {
                        value: v,
                        step_size: 10.0,
                    })
                    .collect::<Vec<_>>()
            })
            .x_axis_formatter(|m, _| format!("{:.0}", m.value as u32))
            .y_axis_formatter(|m, _| format!("{:.0}", m.value))
            .show_x(true)
            .coordinates_formatter(
                Corner::LeftTop,
                egui_plot::CoordinatesFormatter::new(move |plot_point, _plot_bounds| {
                    el_pointer_formatter(plot_point.x, plot_point.y)
                }),
            )
            .legend(Legend::default());

        let az_response = plot_az.show(ui, |plot_ui| {
            plot_ui.set_plot_bounds(egui_plot::PlotBounds::from_min_max(
                [0.0, -5.0],
                [24.7, 365.0],
            ));
            for (name, az_points, _) in lst_plot_data {
                let mut line = Line::new(name.clone(), PlotPoints::from(az_points.clone()));
                if is_sun_track(name) {
                    line = line.color(egui::Color32::YELLOW);
                }
                plot_ui.line(line);
            }
        });

        ui.add_space(-10.0);

        let el_response = plot_el.show(ui, |plot_ui| {
            plot_ui.set_plot_bounds(egui_plot::PlotBounds::from_min_max(
                [0.0, 0.0],
                [24.7, 91.0],
            ));
            for (name, _, el_points) in lst_plot_data {
                let mut line = Line::new(name.clone(), PlotPoints::from(el_points.clone()));
                if is_sun_track(name) {
                    line = line.color(egui::Color32::YELLOW);
                }
                plot_ui.line(line);
            }
        });

        self.lst_plot_rect = Some(az_response.response.rect.union(el_response.response.rect));
    }
}

const DEFAULT_SOURCE_TXT: &str = "# NAME RA_H RA_M RA_S DEC_D DEC_M DEC_S [EPOCH]\n";
const DEFAULT_ANTENNA_SCH: &str = include_str!("../antenna.sch");
const DEFAULT_STATION_TXT: &str = "# NAME X_POS Y_POS Z_POS (ECEF meters)\n";

fn uptimeplot_data_dir() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"));
    #[cfg(not(target_os = "windows"))]
    let home = std::env::var_os("HOME");

    home.map(PathBuf::from).map(|path| path.join(".uptimeplot"))
}

fn runtime_app_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
}

fn ensure_user_data_file(dir: &Path, filename: &str, default_content: &str) -> Option<PathBuf> {
    if fs::create_dir_all(dir).is_err() {
        return None;
    }
    let path = dir.join(filename);
    if !path.exists() && fs::write(&path, default_content).is_err() {
        return None;
    }
    Some(path)
}

fn ensure_text_file_with_header(path: &Path, header: &str) -> Result<(), String> {
    if path.is_file() {
        return Ok(());
    }
    if path.exists() {
        return Err(format!("Path is not a regular file: {}", path.display()));
    }
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create directory {}: {}", parent.display(), e))?;
    }

    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
    {
        Ok(mut file) => file
            .write_all(header.as_bytes())
            .map_err(|e| format!("Failed to initialize {}: {}", path.display(), e)),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(e) => Err(format!("Failed to create {}: {}", path.display(), e)),
    }
}

fn pick_file_dialog(title: &str) -> Result<Option<PathBuf>, String> {
    #[cfg(target_os = "windows")]
    {
        let script = format!(
            "Add-Type -AssemblyName System.Windows.Forms; \
             $d = New-Object System.Windows.Forms.OpenFileDialog; \
             $d.Title = '{}'; \
             if ($d.ShowDialog() -eq [System.Windows.Forms.DialogResult]::OK) {{ $d.FileName }}",
            title.replace('\'', "''")
        );
        return pick_file_from_command("powershell", &["-NoProfile", "-Command", &script]);
    }

    #[cfg(target_os = "macos")]
    {
        let script = format!(
            "POSIX path of (choose file with prompt \"{}\")",
            title.replace('"', "\\\"")
        );
        return pick_file_from_command("osascript", &["-e", &script]);
    }

    #[cfg(target_os = "linux")]
    {
        match pick_file_from_command("zenity", &["--file-selection", "--title", title]) {
            Ok(result) => return Ok(result),
            Err(_) => {
                return pick_file_from_command("kdialog", &["--getopenfilename", ".", "*", title])
            }
        }
    }

    #[allow(unreachable_code)]
    Err("File selection dialog is not supported on this platform.".to_string())
}

fn pick_file_from_command(program: &str, args: &[&str]) -> Result<Option<PathBuf>, String> {
    let output = std::process::Command::new(program)
        .args(args)
        .output()
        .map_err(|e| format!("Failed to open file manager with {}: {}", program, e))?;
    if !output.status.success() {
        return Ok(None);
    }
    let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if path.is_empty() {
        Ok(None)
    } else {
        Ok(Some(PathBuf::from(path)))
    }
}

fn parse_source_tokens(
    parts: &[&str],
    name_idx: usize,
    coord_start: usize,
    line: &str,
) -> Result<Source, String> {
    if parts.len() <= coord_start + 6 {
        return Err(format!("Invalid source line: {}", line));
    }

    let name = parts[name_idx].to_string();
    let ra_h: i32 = parts[coord_start]
        .parse()
        .map_err(|_| format!("Invalid RA hour: {}", line))?;
    let ra_m: i32 = parts[coord_start + 1]
        .parse()
        .map_err(|_| format!("Invalid RA minute: {}", line))?;
    let ra_s: f64 = parts[coord_start + 2]
        .parse()
        .map_err(|_| format!("Invalid RA second: {}", line))?;
    let ra_hours = ra_h as f64 + ra_m as f64 / 60.0 + ra_s / 3600.0;
    let ra_rad = ra_hours * 15.0_f64.to_radians();

    let dec_d_str = parts[coord_start + 3];
    let dec_sign = if dec_d_str.starts_with('-') { '-' } else { '+' };
    let dec_d_raw: i32 = dec_d_str
        .parse()
        .map_err(|_| format!("Invalid Dec degree: {}", line))?;
    let dec_m: i32 = parts[coord_start + 4]
        .parse()
        .map_err(|_| format!("Invalid Dec minute: {}", line))?;
    let dec_s: f64 = parts[coord_start + 5]
        .parse()
        .map_err(|_| format!("Invalid Dec second: {}", line))?;
    let dec_d = dec_d_raw.abs();
    let sign = if dec_sign == '-' { -1.0 } else { 1.0 };
    let dec_deg = sign * (dec_d as f64 + dec_m as f64 / 60.0 + dec_s / 3600.0);
    let dec_rad = dec_deg.to_radians();
    let epoch = parts
        .get(coord_start + 6)
        .copied()
        .unwrap_or("2000.0")
        .to_string();

    Ok(Source {
        name,
        ra_rad,
        dec_rad,
        ra_h,
        ra_m,
        ra_s,
        dec_sign,
        dec_d,
        dec_m,
        dec_s,
        epoch,
    })
}

fn format_source_drg_line(source: &Source) -> String {
    format!(
        "{:<8} {:<8} {:02} {:02} {:08.5} {}{:02} {:02} {:07.4} {}  0  0  0  0",
        source.name,
        source.name,
        source.ra_h,
        source.ra_m,
        source.ra_s,
        source.dec_sign,
        source.dec_d,
        source.dec_m,
        source.dec_s,
        source.epoch
    )
}

fn scan_az_el_for(
    row: &SkdRow,
    source: &Source,
    ant_pos: [f64; 3],
    at_end: bool,
) -> Option<(chrono::NaiveDateTime, f64, f64)> {
    let start = schedule_datetime(row.start_date, &row.start_time).ok()?;
    let time = if at_end {
        start + Duration::seconds(row.duration_sec as i64)
    } else {
        start
    };
    let utc = Utc.from_utc_datetime(&time);
    let ra = source.ra_rad + row.ra_offset_deg.to_radians();
    let dec = (source.dec_rad + row.dec_offset_deg.to_radians())
        .clamp((-90.0_f64).to_radians(), 90.0_f64.to_radians());
    let (az, el, _) = utils::radec2azalt(ant_pos, utc, ra, dec);
    Some((
        time,
        (az + row.az_offset_deg / 60.0).rem_euclid(360.0),
        el + row.el_offset_deg / 60.0,
    ))
}

fn five_point_offset_pattern(offset_deg: f64) -> [(f64, f64); 10] {
    [
        (0.0, 0.0),
        (offset_deg, 0.0),
        (0.0, 0.0),
        (-offset_deg, 0.0),
        (0.0, 0.0),
        (0.0, offset_deg),
        (0.0, 0.0),
        (0.0, -offset_deg),
        (0.0, 0.0),
        (0.0, 0.0),
    ]
}

fn obs_code_station_skd_output_path(
    obs_code: &str,
    station_suffix: &str,
) -> Result<PathBuf, String> {
    if obs_code.trim().is_empty() || obs_code.split_whitespace().count() != 1 {
        return Err("Obscode must be one non-empty word.".to_string());
    }
    Ok(PathBuf::from(format!(
        "{}{}.skd",
        obs_code.trim(),
        station_suffix
    )))
}

fn obs_code_output_path(obs_code: &str) -> Result<PathBuf, String> {
    if obs_code.trim().is_empty() || obs_code.split_whitespace().count() != 1 {
        return Err("Obscode must be one non-empty word.".to_string());
    }
    output_drg_path(obs_code)
}

fn output_drg_path(input: &str) -> Result<PathBuf, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err("Output DRG basename is empty.".to_string());
    }
    let mut path = PathBuf::from(trimmed);
    let has_drg_ext = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.eq_ignore_ascii_case("DRG"))
        .unwrap_or(false);
    if !has_drg_ext {
        path.set_extension("DRG");
    }
    Ok(path)
}

fn five_point_station_offset(station_suffix: &str, row_idx: usize) -> Option<(f64, f64)> {
    let idx = row_idx % 10;
    match station_suffix {
        "32" => Some(match idx {
            0 => (2.0, 0.0),
            1 => (0.0, 0.0),
            2 => (-2.0, 0.0),
            3 => (0.0, 2.0),
            4 => (0.0, -2.0),
            _ => (0.0, 0.0),
        }),
        "34" => Some(match idx {
            5 => (2.0, 0.0),
            6 => (0.0, 0.0),
            7 => (-2.0, 0.0),
            8 => (0.0, 2.0),
            9 => (0.0, -2.0),
            _ => (0.0, 0.0),
        }),
        _ => None,
    }
}

fn format_offset(value: f64, decimals: usize) -> String {
    if value.abs() < 0.5 * 10_f64.powi(-(decimals as i32)) {
        format!("{:.*}", decimals, 0.0)
    } else {
        format!("{:+.*}", decimals, value)
    }
}

fn build_simple_skd_content(
    obs_code: &str,
    sources_section: &str,
    rows: &[SkdRow],
    station_suffix: Option<&str>,
) -> Result<String, String> {
    let mut content = String::new();
    content.push_str(&format!("$EXPER {}\n", obs_code));
    content.push_str("*\n");
    content.push_str(sources_section.trim_end());
    content.push('\n');
    content.push_str("$SKED\n");
    for (idx, row) in rows.iter().enumerate() {
        let start = format_drg_timestamp(row.start_date, &row.start_time)?;
        let (az_offset, el_offset) = if row.include_station_offsets {
            station_suffix
                .and_then(|suffix| five_point_station_offset(suffix, idx))
                .unwrap_or((row.az_offset_deg, row.el_offset_deg))
        } else {
            (row.az_offset_deg, row.el_offset_deg)
        };
        content.push_str(&format!(
            "{:<12} {} {:>6} {:>6} {:>4} {:>4} {:>4}\n",
            row.source_name,
            start,
            row.duration_sec,
            format_offset(az_offset, 1),
            format_offset(el_offset, 1),
            format_offset(row.ra_offset_deg, 1),
            format_offset(row.dec_offset_deg, 1)
        ));
    }
    Ok(content)
}

fn build_new_drg_content(
    obs_code: &str,
    pi_name: &str,
    sources_section: &str,
    sked_section: &str,
) -> String {
    let mut content = String::new();
    content.push_str(&format!("$EXPER {}\n", obs_code));
    content.push_str(&format!("*P.I.: {}\n", pi_name.trim()));
    content.push_str("*Correlator: GICO3\n");
    content.push_str("*\n");
    content.push_str("$PARAM\n");
    content.push_str("SYNCHRONIZE OFF\n");
    content.push_str(sources_section.trim_end());
    content.push('\n');
    content.push_str(DRG_STATIONS_SECTION);
    content.push_str(sked_section.trim_end());
    content.push('\n');
    content.push_str(DRG_HEAD_CODES_SECTION);
    content
}

const DRG_STATIONS_SECTION: &str = "\
$STATIONS\n\
* ANTENNA INFORMATION\n\
A  K YAMAGU32 AZEL   0.00   15.0    0.0    2.0  358.0   15.0    0.0    5.0   85.0   32.0 YM YM\n\
A  L YAMAGU34 AZEL   0.00   12.0    0.0    2.0  358.0   12.0    0.0    5.0   85.0   34.0 Y4 Y4\n\
* STATION POSITION INFORMATION\n\
P YM YAMAGU32 -3502544.5870  3950966.2350  3566381.1920 00000000\n\
P Y4 YAMAGU34 -3502567.5760  3950885.7340  3566449.1150 00000000\n\
* MARK III TERMINALS\n\
T YM YAMAGU32 12\n\
T Y4 YAMAGU34 12\n\
*\n";

const DRG_HEAD_CODES_SECTION: &str = "\
$HEAD\n\
* Head position information for MkIIIA recorders\n\
K S2 11(-350) 21(0) 31(-295) 41(55) 51(-240) 61(110) 71(-185) 81(165) 91(-130)\n\
K S2 A1(220) B1(-75) C1(275)\n\
K S2 11(-350) 21(0) 31(-295) 41(55) 51(-240) 61(110) 71(-185) 81(165) 91(-130)\n\
K S2 A1(220) B1(-75) C1(275)\n\
*\n\
$CODES\n\
*\n\
*\n\
*\n";

fn parse_exper_code(content: &str) -> Option<String> {
    content.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        if parts.next()? == "$EXPER" {
            parts.next().map(|code| code.to_string())
        } else {
            None
        }
    })
}

fn parse_pi_name(content: &str) -> Option<String> {
    content.lines().find_map(|line| {
        let trimmed = line.trim();
        let body = trimmed.strip_prefix('*').unwrap_or(trimmed).trim();
        for prefix in ["P.I.:", "PI:", "P.I.", "PI"] {
            if let Some(value) = body.strip_prefix(prefix) {
                let value = value.trim();
                if !value.is_empty() {
                    return Some(value.to_string());
                }
            }
        }
        None
    })
}

fn section_lines<'a>(content: &'a str, header: &str) -> Result<Vec<&'a str>, String> {
    let lines: Vec<&str> = content.lines().collect();
    let start = lines
        .iter()
        .position(|line| line.trim() == header)
        .ok_or_else(|| format!("{} section not found.", header))?;
    let end = lines
        .iter()
        .enumerate()
        .skip(start + 1)
        .find(|(_, line)| line.trim_start().starts_with('$'))
        .map(|(idx, _)| idx)
        .unwrap_or(lines.len());
    Ok(lines[start + 1..end].to_vec())
}

fn parse_drg_timestamp(value: &str) -> Result<(NaiveDate, String), String> {
    if value.len() != 11 || !value.chars().all(|c| c.is_ascii_digit()) {
        return Err(format!("Invalid DRG timestamp: {}", value));
    }
    let yy: i32 = value[0..2]
        .parse()
        .map_err(|_| format!("Invalid year in timestamp: {}", value))?;
    let year = if yy >= 70 { 1900 + yy } else { 2000 + yy };
    let ordinal: u32 = value[2..5]
        .parse()
        .map_err(|_| format!("Invalid day-of-year in timestamp: {}", value))?;
    let hour: u32 = value[5..7]
        .parse()
        .map_err(|_| format!("Invalid hour in timestamp: {}", value))?;
    let minute: u32 = value[7..9]
        .parse()
        .map_err(|_| format!("Invalid minute in timestamp: {}", value))?;
    let second: u32 = value[9..11]
        .parse()
        .map_err(|_| format!("Invalid second in timestamp: {}", value))?;
    let date = NaiveDate::from_yo_opt(year, ordinal)
        .ok_or_else(|| format!("Invalid date in timestamp: {}", value))?;
    if hour > 23 || minute > 59 || second > 59 {
        return Err(format!("Invalid time in timestamp: {}", value));
    }
    Ok((date, format!("{:02}:{:02}:{:02}", hour, minute, second)))
}

fn format_drg_timestamp(date: NaiveDate, time: &str) -> Result<String, String> {
    let (hour, minute, second) = parse_time_string(time)?;
    Ok(format!(
        "{:02}{:03}{:02}{:02}{:02}",
        date.year().rem_euclid(100),
        date.ordinal(),
        hour,
        minute,
        second
    ))
}

fn schedule_datetime(date: NaiveDate, time: &str) -> Result<chrono::NaiveDateTime, String> {
    let (hour, minute, second) = parse_time_string(time)?;
    date.and_hms_opt(hour, minute, second)
        .ok_or_else(|| format!("Invalid start time: {}", time))
}

fn offset_schedule_time(
    date: NaiveDate,
    time: &str,
    offset_sec: i64,
) -> Result<(NaiveDate, String), String> {
    let start = schedule_datetime(date, time)?;
    let shifted = start + Duration::seconds(offset_sec);
    Ok((
        shifted.date(),
        format!(
            "{:02}:{:02}:{:02}",
            shifted.time().hour(),
            shifted.time().minute(),
            shifted.time().second()
        ),
    ))
}

fn set_skd_row_datetime(row: &mut SkdRow, datetime: chrono::NaiveDateTime) {
    row.start_date = datetime.date();
    row.start_time = format!(
        "{:02}:{:02}:{:02}",
        datetime.time().hour(),
        datetime.time().minute(),
        datetime.time().second()
    );
}

fn parse_time_string(value: &str) -> Result<(u32, u32, u32), String> {
    let trimmed = value.trim();
    let parsed = if trimmed.contains(':') {
        let parts: Vec<&str> = trimmed.split(':').collect();
        if parts.len() != 3 {
            return Err(format!("Invalid time: {}", value));
        }
        (
            parts[0]
                .parse::<u32>()
                .map_err(|_| format!("Invalid hour: {}", value))?,
            parts[1]
                .parse::<u32>()
                .map_err(|_| format!("Invalid minute: {}", value))?,
            parts[2]
                .parse::<u32>()
                .map_err(|_| format!("Invalid second: {}", value))?,
        )
    } else if trimmed.len() == 6 && trimmed.chars().all(|c| c.is_ascii_digit()) {
        (
            trimmed[0..2].parse::<u32>().unwrap(),
            trimmed[2..4].parse::<u32>().unwrap(),
            trimmed[4..6].parse::<u32>().unwrap(),
        )
    } else {
        return Err(format!("Invalid time: {}", value));
    };

    if parsed.0 > 23 || parsed.1 > 59 || parsed.2 > 59 {
        return Err(format!("Invalid time: {}", value));
    }
    Ok(parsed)
}

fn normalize_time_string(value: &str) -> Option<String> {
    parse_time_string(value)
        .ok()
        .map(|(hour, minute, second)| format!("{:02}:{:02}:{:02}", hour, minute, second))
}

fn utc_datetime_from_hour(date: NaiveDate, hour: f64) -> Option<chrono::DateTime<Utc>> {
    if !hour.is_finite() {
        return None;
    }
    let seconds = (hour * 3600.0).round() as i64;
    let day_start = date.and_hms_opt(0, 0, 0)?;
    Some(Utc.from_utc_datetime(&(day_start + Duration::seconds(seconds))))
}

fn format_hour_hms(hour: f64) -> String {
    if !hour.is_finite() {
        return "--:--:--".to_string();
    }

    let wrapped = hour.rem_euclid(24.0);
    let total_seconds = (wrapped * 3600.0).round() as i64;
    let hh = (total_seconds / 3600) % 24;
    let mm = (total_seconds % 3600) / 60;
    let ss = total_seconds % 60;
    format!("{:02}:{:02}:{:02}", hh, mm, ss)
}

fn save_plot_region_png(
    image: &egui::ColorImage,
    rect_points: egui::Rect,
    pixels_per_point: f32,
    path: &PathBuf,
) -> Result<(), String> {
    if image.size[0] == 0 || image.size[1] == 0 {
        return Err("Screenshot image was empty.".to_string());
    }

    let width = image.size[0] as u32;
    let height = image.size[1] as u32;

    let mut raw_pixels = Vec::with_capacity(image.pixels.len() * 4);
    for px in &image.pixels {
        raw_pixels.push(px.r());
        raw_pixels.push(px.g());
        raw_pixels.push(px.b());
        raw_pixels.push(px.a());
    }

    let rgba = image::RgbaImage::from_raw(width, height, raw_pixels)
        .ok_or_else(|| "Failed to build screenshot buffer.".to_string())?;

    // Keep margins so axis labels/ticks around plot frames are included.
    let pad_px = (56.0 * pixels_per_point).round() as i32;
    let mut x = (rect_points.min.x * pixels_per_point).floor() as i32 - pad_px;
    let mut y = (rect_points.min.y * pixels_per_point).floor() as i32 - pad_px;
    let mut w =
        ((rect_points.max.x - rect_points.min.x) * pixels_per_point).ceil() as i32 + pad_px * 2;
    let mut h =
        ((rect_points.max.y - rect_points.min.y) * pixels_per_point).ceil() as i32 + pad_px * 2;

    x = x.clamp(0, width as i32 - 1);
    y = y.clamp(0, height as i32 - 1);
    w = w.max(1).min(width as i32 - x);
    h = h.max(1).min(height as i32 - y);

    let cropped =
        image::imageops::crop_imm(&rgba, x as u32, y as u32, w as u32, h as u32).to_image();
    cropped
        .save(path)
        .map_err(|e| format!("Failed to save {:?}: {}", path, e))
}

fn calendar_ui(ui: &mut egui::Ui, date: &mut NaiveDate) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        if ui.button("<").clicked() {
            let (year, month) = if date.month() == 1 {
                (date.year() - 1, 12)
            } else {
                (date.year(), date.month() - 1)
            };
            *date = date
                .with_year(year)
                .and_then(|d| d.with_month(month))
                .unwrap_or(*date)
                .with_day(1)
                .unwrap_or(*date);
        }
        ui.label(format!(
            "{}  DOY {:03}",
            date.format("%Y - %B"),
            date.ordinal()
        ));
        if ui.button(">").clicked() {
            let (year, month) = if date.month() == 12 {
                (date.year() + 1, 1)
            } else {
                (date.year(), date.month() + 1)
            };
            *date = date
                .with_year(year)
                .and_then(|d| d.with_month(month))
                .unwrap_or(*date)
                .with_day(1)
                .unwrap_or(*date);
        }
    });
    ui.separator();

    let year = date.year();
    let month = date.month();
    let first_day = NaiveDate::from_ymd_opt(year, month, 1).unwrap();
    let weekday_of_first = first_day.weekday().num_days_from_monday();

    egui::Grid::new("calendar_grid").show(ui, |ui| {
        for day in ["Mo", "Tu", "We", "Th", "Fr", "Sa", "Su"] {
            ui.label(day);
        }
        ui.end_row();

        for _ in 0..weekday_of_first {
            ui.label("");
        }

        let mut current_day = first_day;
        while current_day.month() == month {
            let day_num = current_day.day();
            let is_selected = day_num == date.day();
            let button = egui::Button::new(day_num.to_string()).selected(is_selected);

            if ui
                .add(button)
                .on_hover_text(format!(
                    "{}  DOY {:03}",
                    current_day.format("%Y-%m-%d"),
                    current_day.ordinal()
                ))
                .clicked()
            {
                *date = NaiveDate::from_ymd_opt(year, month, day_num).unwrap();
                changed = true;
            }

            if current_day.weekday() == chrono::Weekday::Sun {
                ui.end_row();
            }
            if let Some(next_day) = current_day.succ_opt() {
                current_day = next_day;
            } else {
                break;
            }
        }
    });
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_text_file_is_initialized_without_overwriting_existing_content() {
        let path = std::env::temp_dir().join(format!(
            "uptimeplot-{}-{}.txt",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        ensure_text_file_with_header(&path, DEFAULT_SOURCE_TXT).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), DEFAULT_SOURCE_TXT);

        std::fs::write(&path, "existing\n").unwrap();
        ensure_text_file_with_header(&path, DEFAULT_STATION_TXT).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "existing\n");

        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn sun_position_is_near_the_june_solstice() {
        let time = Utc.with_ymd_and_hms(2026, 6, 21, 12, 0, 0).unwrap();
        let (ra, dec) = utils::sun_radec(time);
        let ra_deg = ra.to_degrees().rem_euclid(360.0);
        let dec_deg = dec.to_degrees();

        assert!((85.0..=95.0).contains(&ra_deg), "RA was {ra_deg}");
        assert!((22.0..=24.5).contains(&dec_deg), "Dec was {dec_deg}");
    }

    #[test]
    fn schedule_offset_rolls_into_the_next_day() {
        let date = NaiveDate::from_ymd_opt(2026, 9, 30).unwrap();
        let (shifted_date, shifted_time) = offset_schedule_time(date, "23:59:30", 90).unwrap();

        assert_eq!(shifted_date, NaiveDate::from_ymd_opt(2026, 10, 1).unwrap());
        assert_eq!(shifted_time, "00:01:00");
    }

    #[test]
    fn schedule_problem_detection_uses_geometry_and_motion_status() {
        let ok = SkdRowStatus {
            start_geometry: "120.0/45.0".to_string(),
            end_geometry: "121.0/45.5".to_string(),
            motion_1: "OK OK 10/30s".to_string(),
            motion_2: "OK OK 12/30s".to_string(),
        };
        assert!(!skd_status_has_problem(&ok));

        let mut failed = ok.clone();
        failed.motion_2 = "OK NO 35/30s".to_string();
        assert!(skd_status_has_problem(&failed));

        failed.motion_2.clear();
        failed.motion_1 = "Load ant".to_string();
        assert!(skd_status_has_problem(&failed));
    }

    #[test]
    fn test_slew_seconds() {
        let antenna = Antenna {
            code: "TEST".to_string(),
            name: "TEST".to_string(),
            pos: [0.0, 0.0, 0.0],
            az_rate_deg_per_min: 60.0, // 1 deg/sec
            az_min_deg: -180.0,
            az_max_deg: 180.0,
            el_rate_deg_per_min: 30.0, // 0.5 deg/sec
            el_min_deg: 0.0,
            el_max_deg: 90.0,
        };

        // No movement: max(0/60*60 + 5, 0/30*60 + 5) + 2 = 7.0
        assert_eq!(antenna.slew_seconds(0.0, 45.0, 0.0, 45.0), Some(7.0));

        // AZ moves 10 deg: 10/60*60 = 10s. 10 + 5 = 15s.
        // EL moves 0 deg: 0 + 5 = 5s.
        // max(15, 5) + 2 = 17.0
        assert_eq!(antenna.slew_seconds(0.0, 45.0, 10.0, 45.0), Some(17.0));

        // AZ moves 0 deg: 0 + 5 = 5s.
        // EL moves 10 deg: 10/30*60 = 20s. 20 + 5 = 25s.
        // max(5, 25) + 2 = 27.0
        assert_eq!(antenna.slew_seconds(0.0, 45.0, 0.0, 55.0), Some(27.0));
    }
}
