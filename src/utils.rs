use chrono::{DateTime, Datelike, Timelike, Utc};

use astro::coords;
use astro::time;
use nav_types::{ECEF, WGS84};
use std::path::Path;
use std::process::Command;

pub fn radec2azalt(
    ant_position: [f64; 3],
    time: DateTime<Utc>,
    obs_ra: f64,
    obs_dec: f64,
) -> (f64, f64, f64) {
    let obs_year = time.year() as i16;
    let obs_month = time.month() as u8;
    let obs_day = time.day() as u8;
    let obs_hour = time.hour() as u8;
    let obs_minute = time.minute() as u8;
    let obs_second = time.second() as f64; // + (time.nanosecond() as f64 / 1_000_000_000.0);

    let decimal_day_calc = obs_day as f64
        + obs_hour as f64 / 24.0
        + obs_minute as f64 / 60.0 / 24.0
        + obs_second as f64 / 24.0 / 60.0 / 60.0;

    let date = time::Date {
        year: obs_year,
        month: obs_month,
        decimal_day: decimal_day_calc,
        cal_type: time::CalType::Gregorian,
    };

    let ecef_position = ECEF::new(ant_position[0], ant_position[1], ant_position[2]);
    let wgs84_position: WGS84<f64> = ecef_position.into();
    let longitude_radian = wgs84_position.longitude_radians();
    let latitude_radian = wgs84_position.latitude_radians();
    let height_meter = wgs84_position.altitude();

    let julian_day = time::julian_day(&date);
    let mean_sidereal = time::mn_sidr(julian_day);
    let hour_angle = coords::hr_angl_frm_observer_long(mean_sidereal, -longitude_radian, obs_ra);

    (
        coords::az_frm_eq(hour_angle, obs_dec, latitude_radian).to_degrees() + 180.0,
        coords::alt_frm_eq(hour_angle, obs_dec, latitude_radian).to_degrees(),
        height_meter,
    )
}

pub fn utc_to_lst_hours(ant_position: [f64; 3], time: DateTime<Utc>) -> f64 {
    let obs_year = time.year() as i16;
    let obs_month = time.month() as u8;
    let obs_day = time.day() as u8;
    let obs_hour = time.hour() as u8;
    let obs_minute = time.minute() as u8;
    let obs_second = time.second() as f64;

    let decimal_day_calc = obs_day as f64
        + obs_hour as f64 / 24.0
        + obs_minute as f64 / 60.0 / 24.0
        + obs_second as f64 / 24.0 / 60.0 / 60.0;

    let date = time::Date {
        year: obs_year,
        month: obs_month,
        decimal_day: decimal_day_calc,
        cal_type: time::CalType::Gregorian,
    };

    let ecef_position = ECEF::new(ant_position[0], ant_position[1], ant_position[2]);
    let wgs84_position: WGS84<f64> = ecef_position.into();
    let longitude_radian = wgs84_position.longitude_radians();

    let julian_day = time::julian_day(&date);
    let mean_sidereal = time::mn_sidr(julian_day);
    let lst_radian = coords::hr_angl_frm_observer_long(mean_sidereal, -longitude_radian, 0.0);
    let wrapped = lst_radian.rem_euclid(2.0 * std::f64::consts::PI);

    wrapped * 24.0 / (2.0 * std::f64::consts::PI)
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
