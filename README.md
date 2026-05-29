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

These files are embedded in the executable and are copied to the user data directory if they do not already exist.

## Build

Linux/macOS native build:

```bash
cargo build --release
```

Run:

```bash
./target/release/uptimeplot
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
