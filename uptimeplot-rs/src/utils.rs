use chrono::{DateTime, Utc, Datelike, Timelike};

use astro::coords;
use astro::time;
use blh::{ellipsoid, GeocentricCoord, GeodeticCoord};
use std::process::Command;
use std::path::Path;

pub fn radec2azalt(ant_position: [f64; 3], time: DateTime<Utc>, obs_ra: f64, obs_dec: f64) -> (f64, f64, f64) {
    let obs_year = time.year() as i16;
    let obs_month = time.month() as u8;
    let obs_day = time.day() as u8;
    let obs_hour = time.hour() as u8;
    let obs_minute = time.minute() as u8;
    let obs_second = time.second() as f64; // + (time.nanosecond() as f64 / 1_000_000_000.0);

    let decimal_day_calc = obs_day as f64 + obs_hour as f64 / 24.0 + obs_minute as f64 / 60.0 / 24.0 + obs_second as f64 / 24.0 / 60.0 / 60.0;

    let date = time::Date {
        year: obs_year,
        month: obs_month,
        decimal_day: decimal_day_calc,
        cal_type: time::CalType::Gregorian,
    };

    let geocentric_coord = GeocentricCoord::new(ant_position[0] as f64, ant_position[1] as f64, ant_position[2] as f64);
    let geodetic_coord: GeodeticCoord<ellipsoid::WGS84> = geocentric_coord.into();
    let longitude_radian = geodetic_coord.lon.0;
    let latitude_radian = geodetic_coord.lat.0;
    let height_meter = geodetic_coord.hgt;

    let julian_day = time::julian_day(&date);
    let mean_sidereal = time::mn_sidr(julian_day);
    let hour_angle = coords::hr_angl_frm_observer_long(mean_sidereal, -longitude_radian, obs_ra as f64);

    (coords::az_frm_eq(hour_angle, obs_dec as f64, latitude_radian).to_degrees() as f64 +180.0, 
     coords::alt_frm_eq(hour_angle, obs_dec as f64, latitude_radian).to_degrees() as f64, 
     height_meter as f64)
}

pub fn open_file_in_external_editor(file_path: &str) -> Result<(), String> {
    let path = Path::new(file_path);
    if !path.exists() {
        return Err(format!("File not found: {}", file_path));
    }

    #[cfg(target_os = "windows")]
    {
        Command::new("cmd")
            .args(&["/C", "start", "", file_path])
            .spawn()
            .map_err(|e| format!("Failed to open file: {}", e))?;
    }

    #[cfg(target_os = "macos")]
    {
        Command::new("open")
            .arg(file_path)
            .spawn()
            .map_err(|e| format!("Failed to open file: {}", e))?;
    }

    #[cfg(target_os = "linux")]
    {
        Command::new("xdg-open")
            .arg(file_path)
            .spawn()
            .map_err(|e| format!("Failed to open file: {}", e))?;
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        return Err("Unsupported operating system for opening files.".to_string());
    }

    Ok(())
}