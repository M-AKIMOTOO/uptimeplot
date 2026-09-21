# uptimeplot

uptimeplot is a Rust/egui desktop tool for checking source visibility and making VLBI observation schedule files.

## Features

- Plot source azimuth/elevation over a UTC day.
- Show polar and LST plots for selected sources.
- Load source, station, and antenna files.
- Build observation schedules in the SKD Table tab.
- Generate a new DRG file plus simple station SKD files.
- Check scan start/end AZ/EL and antenna slew/limit status.
- Generate target/gain-calibrator interleaved schedules.
- Generate five-point observation schedules with station-specific AZ/EL offsets.

## Data Files

At startup, the program creates default data files in:

```text
$HOME/.uptimeplot
```

On Windows this is:

```text
%USERPROFILE%\.uptimeplot
```

The files are:

- `source.txt`
- `antenna.sch`
- `station.txt`

If `source.txt` or `station.txt` is missing, the program creates an empty file
containing only a `#` format header. The embedded default `antenna.sch` is copied
when it is missing.

## Build

Linux/macOS native build:

```bash
cargo build --release
```

The default Linux build supports both X11 and Wayland. To compile only the
window-system backend used by the target environment:

```bash
# X11 only
cargo build --release --no-default-features --features x11

# Wayland only
cargo build --release --no-default-features --features wayland
```

Windows and macOS do not compile the Linux-only backend dependencies. The app
uses eframe's OpenGL (`glow`) renderer; the WGPU/Vulkan renderer is disabled.

Run:

```bash
./target/release/uptimeplot
```

## DRG checks from the terminal

Load a DRG directly into the GUI SKD Table:

```bash
./target/release/uptimeplot --drg schedule.DRG
```

Run the same limits and slew checks without opening the GUI:

```bash
./target/release/uptimeplot --drg schedule.DRG --terminal
```

The schedule is written to standard output as TSV, one row per scan and
antenna. `slew_required_s` and `slew_available_s` show whether the antenna can
reach the next source in time. The `limits_ok`, `slew_ok`, and `scan_ok` columns use
`1` for success and `0` for failure. The command exits with status `1` if any
scan fails, or `2` for an input error.

A bash loop can save one TSV report per DRG:

```bash
for drg in *.DRG; do
    ./target/release/uptimeplot --drg "$drg" --terminal > "${drg%.DRG}.tsv"
done
```

Windows cross-build from Linux, using the GNU target:

```bash
rustup target add x86_64-pc-windows-gnu
cargo build --release --target x86_64-pc-windows-gnu
```

The Windows executable is:

```text
target/x86_64-pc-windows-gnu/release/uptimeplot.exe
```

## SKD Table Outputs

In the SKD Table tab, `Obscode` is used as the output basename. For example, if the obscode is `I25309X`, the program writes:

```text
I25309X.DRG
I25309X32.skd
I25309X34.skd
```

The simple SKD files contain `$SOURCE` and `$SKED` information. SKD `$SKED` rows include:

```text
source name, recording start time, observation time, AZ offset, EL offset, RA offset, Dec offset
```

AZ/EL offsets are arcminutes. RA/Dec offsets are degrees.

## Notes

- The program creates new DRG/SKD files; it does not rewrite the input DRG in place.
- Loading a DRG in the SKD tab keeps the full `source.txt` source list and only adds missing DRG sources.
- Use release builds on low-power machines; debug builds are much slower.
