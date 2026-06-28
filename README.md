# image-chooser

A desktop app for selecting photos from a large collection, with a deliberately simple UI.

## Quick start

```bash
cargo run -- import /path/to/photos
cargo run -- gui
```

Or with a custom project file:

```bash
cargo run -- import family.sqlite /path/to/photos
cargo run -- gui family.sqlite
```

## CLI

| Command | Description |
|---|---|
| `gui [project]` | Launch the selection GUI |
| `import [project] <folder>` | Recursively import images |
| `export [project] <folder>` | Copy selected images to export folder |
| `stats [project]` | Show status counts |
| `help` | Print usage |

Default project path: `~/.local/share/image-chooser/project.sqlite`

## GUI controls

| Action | Button | Key |
|---|---|---|
| Select / print | Ja | `Y` |
| Reject | Nein | `N` |
| Decide later | Später | `L` |
| Undo | Rückgängig | `U` |
| Close zoom | — | `Esc` |
| Zoom in/out | Scroll wheel | — |

## Supported formats

`.jpg`, `.jpeg`, `.png` (v1). HEIC, RAW, video skipped during import.

## Project layout

```
src/main.rs   — CLI entry point
src/lib.rs    — Core library: Project, DB schema, import/export
src/gui.rs    — Iced GUI application
tests/        — Integration tests
```

## Building for Windows

```bash
nix build .#windows
```

## Development

```bash
nix develop
cargo test
cargo run -- gui
```
