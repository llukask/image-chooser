mod gui;

use std::{env, path::PathBuf, process};

use gui::run_gui;
use image_chooser::{Project, StatusCounts, default_project_path};

fn main() -> iced::Result {
    let command = match CliCommand::parse(env::args().skip(1)) {
        Ok(command) => command,
        Err(message) => {
            eprintln!("{message}\n\n{}", usage());
            process::exit(2);
        }
    };

    match command {
        CliCommand::Gui { project } => run_gui(project),
        CliCommand::Help => {
            println!("{}", usage());
            Ok(())
        }
        other => {
            if let Err(error) = run_cli_command(other) {
                eprintln!("Fehler: {error}");
                process::exit(1);
            }
            Ok(())
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CliCommand {
    Gui {
        project: Option<PathBuf>,
    },
    Init {
        project: Option<PathBuf>,
    },
    Import {
        project: Option<PathBuf>,
        folder: PathBuf,
    },
    Export {
        project: Option<PathBuf>,
        target_folder: PathBuf,
    },
    Stats {
        project: Option<PathBuf>,
    },
    Help,
}

impl CliCommand {
    fn parse(args: impl IntoIterator<Item = String>) -> Result<Self, String> {
        let args = args.into_iter().collect::<Vec<_>>();
        let Some(command) = args.first().map(String::as_str) else {
            return Ok(Self::Gui { project: None });
        };

        match command {
            "gui" => match args.as_slice() {
                [_] => Ok(Self::Gui { project: None }),
                [_, project] => Ok(Self::Gui {
                    project: Some(project.into()),
                }),
                _ => Err("Aufruf: image-chooser gui [project.sqlite]".to_owned()),
            },
            "init" => match args.as_slice() {
                [_] => Ok(Self::Init { project: None }),
                [_, project] => Ok(Self::Init {
                    project: Some(project.into()),
                }),
                _ => Err("Aufruf: image-chooser init [project.sqlite]".to_owned()),
            },
            "import" => match args.as_slice() {
                [_, folder] => Ok(Self::Import {
                    project: None,
                    folder: folder.into(),
                }),
                [_, project, folder] => Ok(Self::Import {
                    project: Some(project.into()),
                    folder: folder.into(),
                }),
                _ => Err("Aufruf: image-chooser import [project.sqlite] <photo-folder>".to_owned()),
            },
            "export" => match args.as_slice() {
                [_, target_folder] => Ok(Self::Export {
                    project: None,
                    target_folder: target_folder.into(),
                }),
                [_, project, target_folder] => Ok(Self::Export {
                    project: Some(project.into()),
                    target_folder: target_folder.into(),
                }),
                _ => {
                    Err("Aufruf: image-chooser export [project.sqlite] <target-folder>".to_owned())
                }
            },
            "stats" => match args.as_slice() {
                [_] => Ok(Self::Stats { project: None }),
                [_, project] => Ok(Self::Stats {
                    project: Some(project.into()),
                }),
                _ => Err("Aufruf: image-chooser stats [project.sqlite]".to_owned()),
            },
            "help" | "--help" | "-h" => Ok(Self::Help),
            unknown => Err(format!("Unbekannter Befehl: {unknown}")),
        }
    }
}

fn run_cli_command(command: CliCommand) -> image_chooser::Result<()> {
    match command {
        CliCommand::Init { project } => {
            let project_path = resolve_project_path(project)?;
            let project = Project::open_or_create(project_path)?;
            println!("Projekt bereit: {}", project.path().display());
            println!("Nächster Schritt: image-chooser import <photo-folder>");
        }
        CliCommand::Import { project, folder } => {
            let project_path = resolve_project_path(project)?;
            let project = Project::open_or_create(project_path)?;
            let summary = project.import_folder(folder)?;
            println!("Import abgeschlossen:");
            println!("  gescannt: {}", summary.scanned);
            println!("  neu importiert: {}", summary.imported);
            println!("  schon vorhanden: {}", summary.duplicates);
            println!("  übersprungen/unsupported: {}", summary.unsupported);
            println!("  Lesefehler beim Durchsuchen: {}", summary.walk_errors);
            print_status_counts(project.status_counts()?);
        }
        CliCommand::Export {
            project,
            target_folder,
        } => {
            let project_path = resolve_project_path(project)?;
            let project = Project::open_or_create(project_path)?;
            let counts = project.status_counts()?;
            if counts.unseen > 0 || counts.undecided > 0 {
                eprintln!(
                    "Warnung: Es gibt noch {} ungesehene und {} Später-Bilder. Export läuft trotzdem weiter.",
                    counts.unseen, counts.undecided
                );
            }
            if counts.selected == 0 {
                eprintln!(
                    "Warnung: Es sind keine Bilder ausgewählt. Es wird nur ein Manifest erstellt."
                );
            }

            println!("Exportiere {} ausgewählte Bilder …", counts.selected);
            let summary = project.export_selected(target_folder)?;
            println!("Export abgeschlossen:");
            println!("  Ordner: {}", summary.export_dir.display());
            println!(
                "  Manifest: {}",
                summary.export_dir.join("manifest.csv").display()
            );
            println!("  kopiert: {}", summary.copied);
            println!("  fehlgeschlagen: {}", summary.failed.len());
            for failure in &summary.failed {
                println!(
                    "  FEHLER: {} -> {}: {}",
                    failure.original_path.display(),
                    failure.destination_path.display(),
                    failure.message
                );
            }
        }
        CliCommand::Stats { project } => {
            let project_path = resolve_project_path(project)?;
            let project = Project::open_or_create(project_path)?;
            println!("Projekt: {}", project.path().display());
            print_status_counts(project.status_counts()?);
        }
        CliCommand::Gui { .. } | CliCommand::Help => {}
    }

    Ok(())
}

fn resolve_project_path(project: Option<PathBuf>) -> image_chooser::Result<PathBuf> {
    match project {
        Some(path) => Ok(path),
        None => default_project_path(),
    }
}

fn print_status_counts(counts: StatusCounts) {
    println!("Status:");
    println!("  gesamt: {}", counts.total());
    println!("  ungesehen: {}", counts.unseen);
    println!("  später: {}", counts.undecided);
    println!("  ausgewählt: {}", counts.selected);
    println!("  abgelehnt: {}", counts.rejected);
}

fn usage() -> &'static str {
    "Benutzung:\n  image-chooser                              GUI mit Standardprojekt starten\n  image-chooser gui [project.sqlite]         GUI starten\n  image-chooser init [project.sqlite]        Projektdatei erstellen/öffnen\n  image-chooser import [project.sqlite] <photo-folder>\n  image-chooser stats [project.sqlite]\n  image-chooser export [project.sqlite] <target-folder>\n\nOhne project.sqlite wird das Standardprojekt im Benutzer-Datenordner verwendet.\n\nBeispiel:\n  image-chooser init\n  image-chooser import /pfad/zu/fotos\n  image-chooser gui"
}

#[cfg(test)]
mod tests {
    use super::CliCommand;
    use std::path::PathBuf;

    #[test]
    fn cli_parser_defaults_to_gui_so_nix_run_still_starts_the_app() {
        assert_eq!(
            CliCommand::parse([]).expect("parse empty CLI"),
            CliCommand::Gui { project: None }
        );
    }

    #[test]
    fn cli_parser_accepts_import_arguments_to_avoid_swapping_project_and_folder() {
        assert_eq!(
            CliCommand::parse([
                "import".to_owned(),
                "family.sqlite".to_owned(),
                "photos".to_owned()
            ])
            .expect("parse import CLI"),
            CliCommand::Import {
                project: Some(PathBuf::from("family.sqlite")),
                folder: PathBuf::from("photos"),
            }
        );
    }

    #[test]
    fn cli_parser_uses_default_project_for_one_argument_import() {
        assert_eq!(
            CliCommand::parse(["import".to_owned(), "photos".to_owned()])
                .expect("parse default-project import CLI"),
            CliCommand::Import {
                project: None,
                folder: PathBuf::from("photos"),
            }
        );
    }

    #[test]
    fn cli_parser_accepts_export_arguments_to_make_cli_exports_available() {
        assert_eq!(
            CliCommand::parse([
                "export".to_owned(),
                "family.sqlite".to_owned(),
                "exports".to_owned()
            ])
            .expect("parse export CLI"),
            CliCommand::Export {
                project: Some(PathBuf::from("family.sqlite")),
                target_folder: PathBuf::from("exports"),
            }
        );
    }

    #[test]
    fn cli_parser_uses_default_project_for_one_argument_export() {
        assert_eq!(
            CliCommand::parse(["export".to_owned(), "exports".to_owned()])
                .expect("parse default-project export CLI"),
            CliCommand::Export {
                project: None,
                target_folder: PathBuf::from("exports"),
            }
        );
    }

    #[test]
    fn cli_parser_rejects_incomplete_export_to_avoid_writing_to_the_wrong_folder() {
        assert!(CliCommand::parse(["export".to_owned()]).is_err());
    }
}
