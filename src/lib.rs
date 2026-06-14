use std::{
    cell::RefCell,
    fs,
    path::{Path, PathBuf},
    time::SystemTime,
};

use chrono::{DateTime, Local, SecondsFormat, Utc};
use directories::BaseDirs;
use rusqlite::{Connection, OptionalExtension, params};
use rusqlite_migration::{M, Migrations};
use thiserror::Error;
use walkdir::{DirEntry, WalkDir};

#[derive(Debug, Error)]
pub enum Error {
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("migration error: {0}")]
    Migration(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("csv error: {0}")]
    Csv(#[from] csv::Error),
    #[error("project path has no parent directory: {0}")]
    ProjectPathHasNoParent(PathBuf),
    #[error("invalid image status in database: {0}")]
    InvalidStatus(String),
    #[error("could not determine the user data directory")]
    NoUserDataDir,
    #[error("path has no filename: {0}")]
    PathHasNoFileName(PathBuf),
}

pub type Result<T> = std::result::Result<T, Error>;

pub fn default_project_path() -> Result<PathBuf> {
    let base_dirs = BaseDirs::new().ok_or(Error::NoUserDataDir)?;
    Ok(base_dirs
        .data_local_dir()
        .join("image-chooser")
        .join("project.sqlite"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageStatus {
    Unseen,
    Undecided,
    Selected,
    Rejected,
}

impl ImageStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unseen => "unseen",
            Self::Undecided => "undecided",
            Self::Selected => "selected",
            Self::Rejected => "rejected",
        }
    }

    fn parse(value: &str) -> Result<Self> {
        match value {
            "unseen" => Ok(Self::Unseen),
            "undecided" => Ok(Self::Undecided),
            "selected" => Ok(Self::Selected),
            "rejected" => Ok(Self::Rejected),
            other => Err(Error::InvalidStatus(other.to_owned())),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageChoice {
    pub id: i64,
    pub path: PathBuf,
    pub status: ImageStatus,
    pub position: i64,
    pub ordering_timestamp: Option<String>,
    pub ordering_source: String,
    pub last_error: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

impl ImageChoice {
    pub fn filename(&self) -> String {
        self.path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.path.to_string_lossy().into_owned())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ImportSummary {
    pub scanned: usize,
    pub imported: usize,
    pub duplicates: usize,
    pub unsupported: usize,
    pub walk_errors: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportFailure {
    pub original_path: PathBuf,
    pub destination_path: PathBuf,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportSummary {
    pub export_dir: PathBuf,
    pub copied: usize,
    pub failed: Vec<ExportFailure>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct StatusCounts {
    pub unseen: i64,
    pub undecided: i64,
    pub selected: i64,
    pub rejected: i64,
}

impl StatusCounts {
    pub fn total(self) -> i64 {
        self.unseen + self.undecided + self.selected + self.rejected
    }
}

#[derive(Debug)]
pub struct Project {
    db_path: PathBuf,
    conn: RefCell<Connection>,
}

impl Project {
    pub fn open_or_create(path: impl AsRef<Path>) -> Result<Self> {
        let db_path = path.as_ref().to_path_buf();
        let parent = db_path
            .parent()
            .ok_or_else(|| Error::ProjectPathHasNoParent(db_path.clone()))?;
        fs::create_dir_all(parent)?;
        let existed = db_path.exists();
        if existed {
            backup_existing_project(&db_path)?;
        }

        let mut conn = Connection::open(&db_path)?;
        conn.pragma_update(None, "journal_mode", "DELETE")?;
        migrate(&mut conn)?;

        Ok(Self {
            db_path,
            conn: RefCell::new(conn),
        })
    }

    pub fn path(&self) -> &Path {
        &self.db_path
    }

    pub fn image_count(&self) -> Result<i64> {
        let conn = self.conn.borrow();
        Ok(conn.query_row("SELECT COUNT(*) FROM image_choices", [], |row| row.get(0))?)
    }

    pub fn status_counts(&self) -> Result<StatusCounts> {
        let conn = self.conn.borrow();
        let mut stmt =
            conn.prepare("SELECT status, COUNT(*) FROM image_choices GROUP BY status")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?;

        let mut counts = StatusCounts::default();
        for row in rows {
            let (status, count) = row?;
            match ImageStatus::parse(&status)? {
                ImageStatus::Unseen => counts.unseen = count,
                ImageStatus::Undecided => counts.undecided = count,
                ImageStatus::Selected => counts.selected = count,
                ImageStatus::Rejected => counts.rejected = count,
            }
        }

        Ok(counts)
    }

    pub fn import_folder(&self, folder: impl AsRef<Path>) -> Result<ImportSummary> {
        let folder = folder.as_ref().canonicalize()?;
        let mut summary = ImportSummary::default();
        let mut candidates = Vec::new();

        for entry in WalkDir::new(&folder)
            .follow_links(false)
            .into_iter()
            .filter_entry(|entry| !is_hidden(entry))
        {
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => {
                    summary.walk_errors += 1;
                    continue;
                }
            };

            if !entry.file_type().is_file() {
                continue;
            }

            summary.scanned += 1;
            let path = entry.path();
            if !is_supported_image(path) {
                summary.unsupported += 1;
                continue;
            }

            let canonical_path = path.canonicalize()?;
            let path_string = path_to_db(&canonical_path);
            if self.path_exists(&path_string)? {
                summary.duplicates += 1;
                continue;
            }

            let ordering = ordering_metadata(&canonical_path);
            candidates.push(NewImage {
                path: canonical_path,
                ordering_timestamp: ordering.timestamp,
                ordering_source: ordering.source,
            });
        }

        candidates.sort_by(|left, right| {
            left.ordering_timestamp
                .cmp(&right.ordering_timestamp)
                .then_with(|| path_to_db(&left.path).cmp(&path_to_db(&right.path)))
        });

        let mut conn = self.conn.borrow_mut();
        let tx = conn.transaction()?;
        let max_position: i64 = tx.query_row(
            "SELECT COALESCE(MAX(position), 0) FROM image_choices",
            [],
            |row| row.get(0),
        )?;
        let now = now_string();
        for (index, candidate) in candidates.iter().enumerate() {
            let position = max_position + index as i64 + 1;
            tx.execute(
                "INSERT OR IGNORE INTO image_choices \
                 (path, status, position, ordering_timestamp, ordering_source, created_at, updated_at) \
                 VALUES (?1, 'unseen', ?2, ?3, ?4, ?5, ?5)",
                params![
                    path_to_db(&candidate.path),
                    position,
                    candidate.ordering_timestamp,
                    candidate.ordering_source,
                    now,
                ],
            )?;
            if tx.changes() > 0 {
                summary.imported += 1;
            } else {
                summary.duplicates += 1;
            }
        }
        tx.commit()?;

        Ok(summary)
    }

    pub fn images_by_position(&self) -> Result<Vec<ImageChoice>> {
        self.query_images("SELECT * FROM image_choices ORDER BY position")
    }

    pub fn image_by_id(&self, id: i64) -> Result<Option<ImageChoice>> {
        let conn = self.conn.borrow();
        let mut stmt = conn.prepare("SELECT * FROM image_choices WHERE id = ?1")?;
        stmt.query_row(params![id], image_from_row)
            .optional()?
            .transpose()
    }

    pub fn next_unseen(&self) -> Result<Option<ImageChoice>> {
        let conn = self.conn.borrow();
        let mut stmt = conn.prepare(
            "SELECT * FROM image_choices WHERE status = 'unseen' ORDER BY position LIMIT 1",
        )?;
        stmt.query_row([], image_from_row).optional()?.transpose()
    }

    pub fn next_undecided_after(&self, after_position: Option<i64>) -> Result<Option<ImageChoice>> {
        let conn = self.conn.borrow();
        let mut stmt = conn.prepare(
            "SELECT * FROM image_choices
             WHERE status = 'undecided' AND position > ?1
             ORDER BY position LIMIT 1",
        )?;
        stmt.query_row(params![after_position.unwrap_or(0)], image_from_row)
            .optional()?
            .transpose()
    }

    pub fn set_status(&self, id: i64, status: ImageStatus) -> Result<()> {
        let conn = self.conn.borrow();
        conn.execute(
            "UPDATE image_choices SET status = ?1, updated_at = ?2 WHERE id = ?3",
            params![status.as_str(), now_string(), id],
        )?;
        Ok(())
    }

    pub fn export_selected(&self, target_root: impl AsRef<Path>) -> Result<ExportSummary> {
        let export_root = target_root.as_ref();
        fs::create_dir_all(export_root)?;
        let export_dir = create_fresh_export_dir(export_root)?;
        let selected = self.query_images(
            "SELECT * FROM image_choices WHERE status = 'selected' ORDER BY position",
        )?;

        let mut manifest_rows = Vec::new();
        let mut failures = Vec::new();
        let mut copied = 0;

        for (index, image) in selected.iter().enumerate() {
            let exported_filename = export_filename(index + 1, &image.path)?;
            let destination = export_dir.join(&exported_filename);

            let copy_result = if destination.exists() {
                Err(std::io::Error::new(
                    std::io::ErrorKind::AlreadyExists,
                    "export destination already exists",
                ))
            } else {
                fs::copy(&image.path, &destination).map(|_| ())
            };

            match copy_result {
                Ok(()) => {
                    copied += 1;
                    self.clear_last_error(image.id)?;
                    manifest_rows.push(ManifestRow {
                        exported_filename,
                        original_path: path_to_db(&image.path),
                        status: image.status.as_str().to_owned(),
                        position: image.position,
                    });
                }
                Err(error) => {
                    let message = error.to_string();
                    self.store_last_error(image.id, &message)?;
                    failures.push(ExportFailure {
                        original_path: image.path.clone(),
                        destination_path: destination,
                        message,
                    });
                }
            }
        }

        write_manifest(&export_dir, &manifest_rows)?;

        Ok(ExportSummary {
            export_dir,
            copied,
            failed: failures,
        })
    }

    fn path_exists(&self, path: &str) -> Result<bool> {
        let conn = self.conn.borrow();
        let exists: Option<i64> = conn
            .query_row(
                "SELECT id FROM image_choices WHERE path = ?1",
                params![path],
                |row| row.get(0),
            )
            .optional()?;
        Ok(exists.is_some())
    }

    fn query_images(&self, sql: &str) -> Result<Vec<ImageChoice>> {
        let conn = self.conn.borrow();
        let mut stmt = conn.prepare(sql)?;
        let rows = stmt.query_map([], image_from_row)?;
        let mut images = Vec::new();
        for row in rows {
            images.push(row??);
        }
        Ok(images)
    }

    pub fn clear_last_error(&self, id: i64) -> Result<()> {
        let conn = self.conn.borrow();
        conn.execute(
            "UPDATE image_choices SET last_error = NULL, updated_at = ?1 WHERE id = ?2",
            params![now_string(), id],
        )?;
        Ok(())
    }

    pub fn store_last_error(&self, id: i64, message: &str) -> Result<()> {
        let conn = self.conn.borrow();
        conn.execute(
            "UPDATE image_choices SET last_error = ?1, updated_at = ?2 WHERE id = ?3",
            params![message, now_string(), id],
        )?;
        Ok(())
    }
}

#[derive(Debug)]
struct NewImage {
    path: PathBuf,
    ordering_timestamp: Option<String>,
    ordering_source: String,
}

#[derive(Debug)]
struct OrderingMetadata {
    timestamp: Option<String>,
    source: String,
}

#[derive(Debug)]
struct ManifestRow {
    exported_filename: String,
    original_path: String,
    status: String,
    position: i64,
}

fn migrate(conn: &mut Connection) -> Result<()> {
    let migrations = Migrations::new(vec![M::up(
        "CREATE TABLE image_choices (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            path TEXT NOT NULL UNIQUE,
            status TEXT NOT NULL DEFAULT 'unseen'
                CHECK (status IN ('unseen', 'undecided', 'selected', 'rejected')),
            position INTEGER NOT NULL,
            ordering_timestamp TEXT NULL,
            ordering_source TEXT NOT NULL DEFAULT 'path'
                CHECK (ordering_source IN ('exif', 'mtime', 'path')),
            last_error TEXT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );

        CREATE INDEX idx_image_choices_status_position
        ON image_choices(status, position);

        CREATE INDEX idx_image_choices_ordering
        ON image_choices(ordering_timestamp, path);",
    )]);

    migrations
        .to_latest(conn)
        .map_err(|error| Error::Migration(error.to_string()))?;
    Ok(())
}

fn image_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Result<ImageChoice>> {
    let status: String = row.get("status")?;
    Ok((|| {
        Ok(ImageChoice {
            id: row.get("id")?,
            path: PathBuf::from(row.get::<_, String>("path")?),
            status: ImageStatus::parse(&status)?,
            position: row.get("position")?,
            ordering_timestamp: row.get("ordering_timestamp")?,
            ordering_source: row.get("ordering_source")?,
            last_error: row.get("last_error")?,
            created_at: row.get("created_at")?,
            updated_at: row.get("updated_at")?,
        })
    })())
}

fn backup_existing_project(db_path: &Path) -> Result<()> {
    let timestamp = Local::now().format("%Y%m%d-%H%M%S%.3f");
    let backup = db_path.with_extension(format!("sqlite.bak-{timestamp}"));
    fs::copy(db_path, backup)?;
    Ok(())
}

fn now_string() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn ordering_metadata(path: &Path) -> OrderingMetadata {
    match fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .map(system_time_to_string)
    {
        Some(timestamp) => OrderingMetadata {
            timestamp: Some(timestamp),
            source: "mtime".to_owned(),
        },
        None => OrderingMetadata {
            timestamp: None,
            source: "path".to_owned(),
        },
    }
}

fn system_time_to_string(time: SystemTime) -> String {
    let datetime: DateTime<Utc> = time.into();
    datetime.to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn is_supported_image(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "jpg" | "jpeg" | "png"
            )
        })
        .unwrap_or(false)
}

fn is_hidden(entry: &DirEntry) -> bool {
    entry
        .file_name()
        .to_str()
        .map(|name| name.starts_with('.'))
        .unwrap_or(false)
}

fn path_to_db(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn create_fresh_export_dir(root: &Path) -> Result<PathBuf> {
    let timestamp = Local::now().format("%Y%m%d-%H%M%S").to_string();
    for suffix in 0..1000 {
        let name = if suffix == 0 {
            format!("image-chooser-export-{timestamp}")
        } else {
            format!("image-chooser-export-{timestamp}-{suffix:03}")
        };
        let candidate = root.join(name);
        match fs::create_dir(&candidate) {
            Ok(()) => return Ok(candidate),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }

    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "could not create a unique timestamped export directory",
    )
    .into())
}

fn export_filename(index: usize, original_path: &Path) -> Result<String> {
    let original_name = original_path
        .file_name()
        .ok_or_else(|| Error::PathHasNoFileName(original_path.to_path_buf()))?
        .to_string_lossy();
    Ok(format!("{index:06}_{original_name}"))
}

fn write_manifest(export_dir: &Path, rows: &[ManifestRow]) -> Result<()> {
    let manifest_path = export_dir.join("manifest.csv");
    let mut writer = csv::Writer::from_path(manifest_path)?;
    writer.write_record(["exported_filename", "original_path", "status", "position"])?;
    for row in rows {
        writer.write_record([
            row.exported_filename.as_str(),
            row.original_path.as_str(),
            row.status.as_str(),
            &row.position.to_string(),
        ])?;
    }
    writer.flush()?;
    Ok(())
}
