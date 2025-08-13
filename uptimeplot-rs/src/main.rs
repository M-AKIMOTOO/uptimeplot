#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use eframe::egui;
use egui_plot::{GridMark, Legend, Line, Plot, PlotPoints, Corner, Points};
use chrono::{Datelike, NaiveDate, Utc, TimeZone};
use std::fs;
use std::path::PathBuf;
use std::io::{BufReader, BufRead};
use image;
use bytemuck;
use clap::{CommandFactory, Parser};

mod utils;

#[derive(PartialEq)]
enum AppTab {
    UptimePlotters,
    Parameters,
    PolarPlot,
}

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct CliArgs {
    /// Path to the station.txt file
    #[arg(long)]
    station_path: Option<PathBuf>,

    /// Path to the source.txt file
    #[arg(long)]
    source_path: Option<PathBuf>,
}

fn main() -> Result<(), eframe::Error> {
    let cli_args = CliArgs::parse();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1280.0, 720.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Uptime Plotter",
        options,
        Box::new(move |cc| { // Use move to capture cli_args
            let app = Box::new(UptimePlotApp::new(cli_args)); // Call new constructor

            // Increase font size
            let mut style = (*cc.egui_ctx.style()).clone();
            for (_text_style, font_id) in style.text_styles.iter_mut() {
                font_id.size *= 1.5; // Increase by 1.5 times
            }
            style.visuals.panel_fill = egui::Color32::TRANSPARENT;
            cc.egui_ctx.set_style(style);

            app
        }),
    )
}

struct Station {
    name: String,
    pos: [f64; 3],
}

#[derive(Clone)]
struct Source {
    name: String,
    ra_rad: f64,
    dec_rad: f64,
}

struct UptimePlotApp {
    stations: Vec<Station>,
    selected_station: usize,
    selected_date: NaiveDate,
    station_file_path: String,
    source_file_path: String,
    sources: Vec<(Source, bool)> ,
    plot_data: Vec<(String, Vec<[f64; 2]>, Vec<[f64; 2]>)>, 
    error_msg: Option<String>,
    show_calendar: bool,
    search_query: String,
    selected_tab: AppTab,
}



impl UptimePlotApp {
    fn new(cli_args: CliArgs) -> Self {
        let cargo_manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));

        // Determine station_file_path
        let station_file_path = cli_args.station_path.unwrap_or_else(|| {
            cargo_manifest_dir.join("station.txt")
        });

        // Determine source_file_path
        let source_file_path = cli_args.source_path.unwrap_or_else(|| {
            cargo_manifest_dir.join("source.txt")
        });

        let stations: Vec<Station> = {
            let mut stations_vec = Vec::new();
            if let Ok(file) = fs::File::open(&station_file_path) {
                let reader = BufReader::new(file);
                for line in reader.lines() {
                    if let Ok(line) = line {
                        let parts: Vec<&str> = line.trim().split_whitespace().collect();
                        if parts.len() == 4 {
                            if let (Ok(pos_x), Ok(pos_y), Ok(pos_z)) = (parts[1].parse::<f64>(), parts[2].parse::<f64>(), parts[3].parse::<f64>()) {
                                stations_vec.push(Station { name: parts[0].to_string(), pos: [pos_x, pos_y, pos_z] });
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
            stations.iter().position(|s| s.name == "YAMAGU32").unwrap_or(0)
        };

        Self {
            stations,
            selected_station: default_station_idx,
            selected_date: Utc::now().date_naive(),
            station_file_path: station_file_path.to_str().unwrap_or_default().to_string(),
            source_file_path: source_file_path.to_str().unwrap_or_default().to_string(),
            sources: Vec::new(),
            plot_data: Vec::new(),
            error_msg: None,
            show_calendar: false,
            search_query: String::new(),
            selected_tab: AppTab::UptimePlotters,
        }
    }
}

impl eframe::App for UptimePlotApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.show_calendar_window(ctx);

        if let Some(event) = ctx.input(|i| i.events.iter().find_map(|e| {
            if let egui::Event::Screenshot { image, .. } = e {
                Some(image.clone())
            } else {
                None
            }
        })) {
            let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
            path.push("plot_screenshot.png");
            if let Err(e) = image::save_buffer(
                &path,
                bytemuck::cast_slice(event.pixels.as_slice()),
                event.width() as u32,
                event.height() as u32,
                image::ColorType::Rgba8,
            ) {
                self.error_msg = Some(format!("Failed to save screenshot: {}", e));
            } else {
                self.error_msg = Some(format!("Screenshot saved to {:?}", path));
            }
        }

        egui::TopBottomPanel::top("top_panel").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.selected_tab, AppTab::Parameters, "Parameters");
                ui.selectable_value(&mut self.selected_tab, AppTab::UptimePlotters, "Uptime Plotters");
                ui.selectable_value(&mut self.selected_tab, AppTab::PolarPlot, "Polar Plot");
            });
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            match self.selected_tab {
                AppTab::UptimePlotters => self.ui_uptime_plotters_tab(ui),
                AppTab::Parameters => self.ui_parameters_tab(ui),
                AppTab::PolarPlot => self.ui_polar_plot_tab(ui),
            }
        });
    }
}

impl UptimePlotApp {
    fn load_sources(&mut self) -> Result<(), String> {
        let source_content = fs::read_to_string(&self.source_file_path)
            .map_err(|e| format!("Failed to read source file: {}", e))?;

        let mut sources = Vec::new();
        for line in source_content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('*') { continue; }
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() < 7 { continue; }

            let name = parts[0].to_string();
            let ra_h: f64 = parts[1].parse().map_err(|_| format!("Invalid RA hour: {}", line))?;
            let ra_m: f64 = parts[2].parse().map_err(|_| format!("Invalid RA minute: {}", line))?;
            let ra_s: f64 = parts[3].parse().map_err(|_| format!("Invalid RA second: {}", line))?;
            let ra_hours = ra_h + ra_m / 60.0 + ra_s / 3600.0;
            let ra_rad = ra_hours * 15.0 * (std::f64::consts::PI / 180.0);

            let dec_d_str = parts[4];
            let sign = if dec_d_str.starts_with('-') { -1.0 } else { 1.0 };
            let dec_d: f64 = dec_d_str.parse().map_err(|_| format!("Invalid Dec degree: {}", line))?;
            let dec_m: f64 = parts[5].parse().map_err(|_| format!("Invalid Dec minute: {}", line))?;
            let dec_s: f64 = parts[6].parse().map_err(|_| format!("Invalid Dec second: {}", line))?;
            let dec_deg = sign * (dec_d.abs() + dec_m / 60.0 + dec_s / 3600.0);
            let dec_rad = dec_deg.to_radians();

            sources.push((Source { name, ra_rad, dec_rad }, false));
        }
        self.sources = sources;
        self.plot_data.clear();
        Ok(())
    }

    fn load_stations(&mut self) -> Result<(), String> {
        let station_content = fs::read_to_string(&self.station_file_path)
            .map_err(|e| format!("Failed to read station file: {}", e))?;

        let mut stations_vec = Vec::new();
        for line in station_content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('*') { continue; }
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() == 4 {
                if let (Ok(pos_x), Ok(pos_y), Ok(pos_z)) = (parts[1].parse::<f64>(), parts[2].parse::<f64>(), parts[3].parse::<f64>()) {
                    stations_vec.push(Station { name: parts[0].to_string(), pos: [pos_x, pos_y, pos_z] });
                } else {
                    return Err(format!("Invalid number format in station file: {}", line));
                }
            } else {
                return Err(format!("Invalid line format in station file: {}", line));
            }
        }
        self.stations = stations_vec;
        // Reset selected_station if the current one is no longer valid
        if self.selected_station >= self.stations.len() {
            self.selected_station = 0;
        }
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

        let station = &self.stations[self.selected_station];
        let ant_pos = station.pos;
        let mut new_plot_data = Vec::new();

        for (source, selected) in &self.sources {
            if !*selected { continue; }

            let mut full_day_points = Vec::new();
            for i in (0..=(24 * 60)).step_by(3) { // 3 minute intervals
                let hour_float = (i as f64) / 60.0;
                let h = (i / 60) as u32;
                let m = (i % 60) as u32;

                if let Some(time) = self.selected_date.and_hms_opt(h, m, 0) {
                    let datetime_utc = Utc.from_utc_datetime(&time);
                    let (az, el, _) = utils::radec2azalt(ant_pos, datetime_utc, source.ra_rad, source.dec_rad);
                    full_day_points.push((hour_float, az, el));
                }
            }

            let mut az_points = Vec::new();
            let mut el_points = Vec::new();

            if let Some(last_point) = full_day_points.get(0) {
                if last_point.2 >= 0.0 {
                    az_points.push([last_point.0, last_point.1]);
                    el_points.push([last_point.0, last_point.2]);
                }

                for &point in full_day_points.iter().skip(1) {

                    let (hour, az, el) = point;

                    az_points.push([hour, az]);

                    if el >= 0.0 {
                        el_points.push([hour, el]);
                    } else {
                        el_points.push([hour, f64::NAN]);
                    }
                }
            }
            new_plot_data.push((source.name.clone(), az_points, el_points));
        }
        self.plot_data = new_plot_data;
    }

    fn show_calendar_window(&mut self, ctx: &egui::Context) {
        if self.show_calendar {
            let mut open = true;
            egui::Window::new("Select Date").open(&mut open).collapsible(false).resizable(false).show(ctx, |ui| {
                if calendar_ui(ui, &mut self.selected_date) {
                    self.show_calendar = false;
                }
            });
            if !open {
                self.show_calendar = false;
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
            header.push_str(&format!( ",{},{}", name, name)); // Add source name twice for AZ and EL
            if i == 0 { // Assuming time points are common for all sources
                time_points = az_points.iter().map(|p| p[0]).collect();
            }
        }
        csv_content.push_str(&header);
        csv_content.push_str("\n");

        // Populate data rows
        for &time in &time_points {
            let mut row = format!( "{:.2}", time); // Format time to 2 decimal places
            for (_, az_points, el_points) in &self.plot_data {
                // Find the corresponding az and el for this time
                let az_val = az_points.iter().find(|p| (p[0] - time).abs() < 1e-6).map_or("".to_string(), |p| format!( "{:.1}", p[1]));
                let el_val = el_points.iter().find(|p| (p[0] - time).abs() < 1e-6).map_or("".to_string(), |p| format!( "{:.1}", p[1]));
                row.push_str(&format!( ",{},{}", az_val, el_val));
            }
            csv_content.push_str(&row);
            csv_content.push_str("\n");
        }

        let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        path.push("plot_data.csv");
        fs::write(&path, csv_content).map_err(|e| format!( "Failed to save CSV file: {}", e))?;
        Ok(())
    }

    fn ui_uptime_plotters_tab(&mut self, ui: &mut egui::Ui) {
        let az_pointer_formatter = |x: f64, y: f64| format!("Time: {:02}:{:02}\nAz: {:.1}°", x as u32, (x.fract() * 60.0) as u32, y);
        let el_pointer_formatter = |x: f64, y: f64| format!("Time: {:02}:{:02}\nEl: {:.1}°", x as u32, (x.fract() * 60.0) as u32, y);

        let plot_az = Plot::new("az_plot").width(ui.available_width()).height(ui.available_height() / 2.0)
            .y_axis_label("Azimuth (deg)")
            .y_axis_width(4)
            .include_x(0.0).include_x(24.0)
            .include_y(0.0).include_y(360.0)
            .allow_drag(false).allow_zoom(false).allow_scroll(false)
            .x_axis_label("") // Re-added
            .x_axis_formatter(|_,_,_| "".to_string()) // Re-added
            .x_grid_spacer(|_input| {[0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0, 13.0, 14.0, 15.0, 16.0, 17.0, 18.0, 19.0, 20.0, 21.0, 22.0, 23.0, 24.0].into_iter().map(|v| GridMark { value: v, step_size: 3.0 }).collect::<Vec<_>>()})
            .y_grid_spacer(|_input| {[0.0, 30.0, 60.0, 90.0, 120.0, 150.0, 180.0, 210.0, 240.0, 270.0, 300.0, 330.0, 360.0].into_iter().map(|v| GridMark { value: v, step_size: 30.0 }).collect::<Vec<_>>()})
            .y_axis_formatter(|m, _, _| format!( "{:.0}", m.value as i32)).show_y(true)
            .coordinates_formatter(Corner::LeftTop, egui_plot::CoordinatesFormatter::new(move |plot_point, _plot_bounds| az_pointer_formatter(plot_point.x, plot_point.y)))
            .legend(Legend::default());

        let plot_el = Plot::new("el_plot").width(ui.available_width()).height(ui.available_height() / 2.0)
            .x_axis_label("Time (UT)")
            .y_axis_label("Elevation (deg)")
            .y_axis_width(4)
            .include_x(0.0).include_x(24.0)
            .include_y(0.0).include_y(90.0)
            .allow_drag(false).allow_zoom(false).allow_scroll(false)
            .x_grid_spacer(|_input| {[0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0, 13.0, 14.0, 15.0, 16.0, 17.0, 18.0, 19.0, 20.0, 21.0, 22.0, 23.0, 24.0].into_iter().map(|v| GridMark { value: v, step_size: 3.0 }).collect::<Vec<_>>()})
            .x_axis_formatter(|m, _, _| format!( "{:.0}", m.value as u32)).show_x(true)
            .coordinates_formatter(Corner::LeftTop, egui_plot::CoordinatesFormatter::new(move |plot_point, _plot_bounds| el_pointer_formatter(plot_point.x, plot_point.y)))
            .legend(Legend::default());

        plot_az.show(ui, |plot_ui| {
            plot_ui.set_plot_bounds(egui_plot::PlotBounds::from_min_max([0.0, 0.0], [24.7, 360.0]));
            for (name, az_points, _) in &self.plot_data {
                plot_ui.line(Line::new(PlotPoints::from(az_points.clone())).name(name));
            }
        });

        ui.add_space(-10.0);

        plot_el.show(ui, |plot_ui| {
            plot_ui.set_plot_bounds(egui_plot::PlotBounds::from_min_max([0.0, 0.0], [24.7, 90.0]));
            for (name, _, el_points) in &self.plot_data {
                plot_ui.line(Line::new(PlotPoints::from(el_points.clone())).name(name));
            }
        });
    }

    fn ui_parameters_tab(&mut self, ui: &mut egui::Ui) {
        ui.heading("Parameters");
        ui.add_space(10.0);

        ui.columns(2, |columns| {
            // --- Left Column: Settings and File Formats ---
            egui::ScrollArea::vertical().show(&mut columns[0], |ui| {
                // --- Station Settings ---
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.heading("📡 Station Settings");
                    ui.add_space(5.0);
                    egui::Grid::new("station_grid").num_columns(2).spacing([40.0, 4.0]).striped(true).show(ui, |ui| {
                        ui.label("Station:");
                        egui::ComboBox::new("station_combo", "")
                            .selected_text(if self.stations.is_empty() { "No stations loaded" } else { &self.stations[self.selected_station].name })
                            .show_ui(ui, |ui| {
                                if self.stations.is_empty() {
                                    ui.label("Load stations from station.txt");
                                } else {
                                    for (i, station) in self.stations.iter().enumerate() {
                                        ui.selectable_value(&mut self.selected_station, i, &station.name);
                                    }
                                }
                            });
                        ui.end_row();

                        ui.label("Station File:");
                        ui.horizontal(|ui| {
                            ui.text_edit_singleline(&mut self.station_file_path);
                            if ui.button("Load").clicked() {
                                match self.load_stations() {
                                    Ok(_) => self.error_msg = None,
                                    Err(e) => self.error_msg = Some(e),
                                }
                            }
                            if ui.button("Open").clicked() {
                                match utils::open_file_in_external_editor(&self.station_file_path) {
                                    Ok(_) => self.error_msg = None,
                                    Err(e) => self.error_msg = Some(e),
                                }
                            }
                        });
                        ui.end_row();
                    });
                });
                ui.add_space(10.0);

                // --- Observation Settings ---
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.heading("Observation Settings");
                    ui.add_space(5.0);
                    egui::Grid::new("obs_grid").num_columns(2).spacing([40.0, 4.0]).striped(true).show(ui, |ui| {
                        ui.label("Observation Date:");
                        if ui.button(self.selected_date.format("%Y-%m-%d").to_string()).clicked() {
                            self.show_calendar = !self.show_calendar;
                        }
                        ui.end_row();
                    });
                });
                ui.add_space(10.0);

                // --- Source Settings ---
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.heading("🔭 Source Settings");
                    ui.add_space(5.0);
                    egui::Grid::new("source_settings_grid").num_columns(2).spacing([40.0, 4.0]).striped(true).show(ui, |ui| {
                        ui.label("Source List File:");
                        ui.horizontal(|ui| {
                            ui.text_edit_singleline(&mut self.source_file_path);
                            if ui.button("Load").clicked() {
                                match self.load_sources() {
                                    Ok(_) => self.error_msg = None,
                                    Err(e) => self.error_msg = Some(e),
                                }
                            }
                            if ui.button("Open").clicked() {
                                match utils::open_file_in_external_editor(&self.source_file_path) {
                                    Ok(_) => self.error_msg = None,
                                    Err(e) => self.error_msg = Some(e),
                                }
                            }
                        });
                        ui.end_row();

                        ui.label("Search Filter:");
                        ui.add(egui::TextEdit::singleline(&mut self.search_query).frame(true));
                        ui.end_row();
                    });

                    ui.separator();
                    ui.label("Select Sources to Plot:");
                    ui.horizontal(|ui|{
                        if ui.button("Plot Selected").clicked() {
                            self.calculate_plots();
                        }
                        if ui.button("Reset Source Selection").clicked() {
                            for (_, selected) in &mut self.sources {
                                *selected = false;
                            }
                        }
                    });

                    egui::ScrollArea::vertical().max_height(200.0).show(ui, |ui| {
                        if self.sources.is_empty() {
                            ui.label("(No sources loaded)");
                        } else {
                            egui::Grid::new("source_grid").show(ui, |ui| {
                                let mut displayed_count = 0;
                                for (_i, (source, selected)) in self.sources.iter_mut().enumerate() {
                                    if self.search_query.is_empty() || source.name.to_lowercase().contains(&self.search_query.to_lowercase()) {
                                        ui.checkbox(selected, &source.name);
                                        displayed_count += 1;
                                        if displayed_count % 8 == 0 {
                                            ui.end_row();
                                        }
                                    }
                                }
                            });
                        }
                    });
                });
                ui.add_space(10.0);

                // --- File Formats (Moved here) ---
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.heading("📄 File Format Information");
                    ui.add_space(5.0);
                    ui.label("station.txt format (ECEF): NAME X_POS Y_POS Z_POS");
                    ui.label("e.g. YAMAGU32 -3502544.587 3950966.235 3566381.192");
                    ui.separator();
                    ui.label("source.txt format: NAME  RA_H  RA_M  RA_S  DEC_D  DEC_M  DEC_S");
                    ui.label("e.g. 3C273  12 29 06.7 +02 03 08.6");
                });

                if let Some(err) = &self.error_msg {
                    ui.add_space(10.0);
                    ui.colored_label(egui::Color32::RED, err);
                }
            });

            // --- Right Column: Usage Only ---
            egui::ScrollArea::vertical().show(&mut columns[1], |ui| {
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.heading("ℹProgram Usage");
                    ui.add_space(5.0);
                    let mut help_text = CliArgs::command().render_help().to_string();
                    // Revert to ui.code() for now
                    ui.add(egui::TextEdit::multiline(&mut help_text)
                        .desired_width(f32::INFINITY)
                        .interactive(false)
                        .font(egui::TextStyle::Monospace));
                    ui.label("1. Push the Load button in Source settings.");
                    ui.label("2. Select some targets for drawing an uptime plot.");
                    ui.label("3. Push the Plot Selected button in Source settings.");
                    ui.label("4. Push the Uptime Plotters button in the upper left corner in this tab.");
                    ui.label("");
                    ui.label("To create a new uptime plot graph, first, push the \"Reset Source Selection\" button in Source Settings to clear previous selections. Then, you can repeat steps 1 to 4 to generate a new plot.");
                    ui.label("");
                    ui.label("If you want to edit the data files for sources or stations, please push the \"Open\" buttons in Station Settings and Source Settings.");
                });
            });
        });
    }

    fn ui_polar_plot_tab(&mut self, ui: &mut egui::Ui) {
        //ui.heading("Polar Plot");

        let plot = Plot::new("polar_plot")
            .width(ui.available_width()) // Added
            .height(ui.available_height()) // Added
            .data_aspect(1.0) // Ensure circular aspect ratio
            .view_aspect(1.0) // Ensure circular aspect ratio
            .include_x(-1.0).include_x(1.0) // Cartesian coordinates for polar plot
            .include_y(-1.0).include_y(1.0) // Cartesian coordinates for polar plot
            .center_x_axis(true)
            .center_y_axis(true)
            .show_x(false) // Hide Cartesian x-axis
            .show_y(false) // Hide Cartesian y-axis
            .x_grid_spacer(|_input| vec![]) // Disable x-grid
            .y_grid_spacer(|_input| vec![]) // Disable y-grid
            .legend(Legend::default());

        plot.show(ui, |plot_ui| {
            // Draw circles for elevation levels (e.g., 0, 30, 60, 90)
            // 90 deg el is center (radius 0), 0 deg el is outer edge (radius 1)
            // So, radius = (90 - el) / 90
            for el_level in [0.0, 15.0, 30.0, 45.0, 60.0, 75.0, 90.0] {
                let radius = (90.0 - el_level) / 90.0;
                if radius >= 0.0 { // Ensure radius is non-negative
                    let num_segments = 100;
                    let mut circle_points = Vec::new();
                    for i in 0..=num_segments {
                        let angle = i as f64 * 2.0 * std::f64::consts::PI / num_segments as f64;
                        let x = radius * angle.cos();
                        let y = radius * angle.sin();
                        circle_points.push([x, y]);
                    }
                    plot_ui.line(Line::new(PlotPoints::from(circle_points)).stroke(egui::Stroke::new(2.0, egui::Color32::DARK_GRAY)));

                    // Add elevation labels
                    if el_level != 90.0 { // Don't label the center point
                        let label_text = format!("{:.0}°", el_level);
                        // Position the label slightly inside the circle, at 0 azimuth (North)
                        let label_x = radius * (72.0f64).to_radians().cos();
                        let label_y = radius * (72.0f64).to_radians().sin();
                        plot_ui.text(egui_plot::Text::new(egui_plot::PlotPoint::new(label_x, label_y), label_text).color(egui::Color32::DARK_GRAY));
                    }
                }
            }

            // Draw radial lines for azimuth (e.g., 0, 90, 180, 270)
            for az_level in [0.0, 45.0, 90.0, 135.0, 180.0, 225.0, 270.0, 315.0] {
                let angle_rad = (90.0f64 - az_level).to_radians(); // Adjust for egui_plot's 0 deg at positive x-axis, clockwise
                let x = 1.0 * angle_rad.cos();
                let y = 1.0 * angle_rad.sin();
                plot_ui.line(Line::new(PlotPoints::from(vec![[0.0, 0.0], [x, y]])).stroke(egui::Stroke::new(2.0, egui::Color32::DARK_GRAY)));

                // Add azimuth labels
                let label_text = format!("{:.0}°", az_level);
                plot_ui.text(egui_plot::Text::new(egui_plot::PlotPoint::new(x * 1.1, y * 1.1), label_text).color(egui::Color32::DARK_GRAY));
            }

            for (name, az_points, el_points) in &self.plot_data {
                let mut polar_points = Vec::new();
                for i in 0..az_points.len() {
                    let az = az_points[i][1]; // Azimuth in degrees
                    let el = el_points[i][1]; // Elevation in degrees

                    if !el.is_nan() && el >= 0.0 { // Only plot if elevation is not NaN AND is >= 0
                        // Convert az/el to Cartesian for egui_plot
                        // Azimuth: 0-360 deg, clockwise positive. egui_plot's angle is counter-clockwise from positive x-axis.
                        // So, convert az to angle_rad: (90 - az) deg to radians.
                        let angle_rad = (90.0f64 - az).to_radians();
                        // Elevation: 90 deg (zenith) -> radius 0, 0 deg (horizon) -> radius 1.
                        let radius = (90.0 - el) / 90.0;

                        let x = radius * angle_rad.cos();
                        let y = radius * angle_rad.sin();
                        polar_points.push([x, y]);
                    }
                }
                if !polar_points.is_empty() {
                    plot_ui.points(Points::new(PlotPoints::from(polar_points)).name(name.clone()));
                }
            }
        });
    }
}

fn calendar_ui(ui: &mut egui::Ui, date: &mut NaiveDate) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        if ui.button("<").clicked() { 
            let (year, month) = if date.month() == 1 { (date.year() - 1, 12) } else { (date.year(), date.month() - 1) };
            *date = date.with_year(year).and_then(|d| d.with_month(month)).unwrap_or(*date).with_day(1).unwrap_or(*date);
        }
        ui.label(date.format("%Y - %B").to_string());
        if ui.button(">").clicked() { 
            let (year, month) = if date.month() == 12 { (date.year() + 1, 1) } else { (date.year(), date.month() + 1) };
            *date = date.with_year(year).and_then(|d| d.with_month(month)).unwrap_or(*date).with_day(1).unwrap_or(*date);
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

            if ui.add(button).clicked() {
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
