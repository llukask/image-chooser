use std::{fs, io::Cursor, path::Path};

use exif::{Field, In, Tag, Value, experimental::Writer as ExifWriter};
use filetime::FileTime;
use image_chooser::{ImageStatus, Project};
use tempfile::TempDir;

fn write_photo(path: &Path) {
    fs::create_dir_all(path.parent().expect("test path has parent")).expect("create parent dir");
    fs::write(
        path,
        b"not a real jpeg, but enough for import/export core tests",
    )
    .expect("write test photo");
}

fn write_photo_with_exif_datetime_original(path: &Path, datetime: &str) {
    fs::create_dir_all(path.parent().expect("test path has parent")).expect("create parent dir");
    let field = Field {
        tag: Tag::DateTimeOriginal,
        ifd_num: In::PRIMARY,
        value: Value::Ascii(vec![datetime.as_bytes().to_vec()]),
    };
    let mut writer = ExifWriter::new();
    writer.push_field(&field);
    let mut exif_data = Cursor::new(Vec::new());
    writer
        .write(&mut exif_data, false)
        .expect("encode test exif data");

    let mut app1_payload = b"Exif\0\0".to_vec();
    app1_payload.extend(exif_data.into_inner());
    let app1_len = app1_payload.len() + 2;
    assert!(app1_len <= u16::MAX as usize, "test APP1 segment fits");

    let mut jpeg = vec![0xff, 0xd8, 0xff, 0xe1];
    jpeg.extend((app1_len as u16).to_be_bytes());
    jpeg.extend(app1_payload);
    jpeg.extend([0xff, 0xd9]);
    fs::write(path, jpeg).expect("write test jpeg with exif");
}

fn set_mtime(path: &Path, unix_seconds: i64) {
    filetime::set_file_mtime(path, FileTime::from_unix_time(unix_seconds, 0))
        .expect("set test photo mtime");
}

#[test]
fn migrations_create_the_image_choices_schema_to_avoid_runtime_project_file_failures() {
    let temp = TempDir::new().expect("create temp dir");
    let db_path = temp.path().join("project.sqlite");

    let project = Project::open_or_create(&db_path).expect("open project");

    assert_eq!(project.image_count().expect("count images"), 0);
}

#[test]
fn import_reimport_and_queue_rules_preserve_decisions_and_stable_positions() {
    let temp = TempDir::new().expect("create temp dir");
    let project =
        Project::open_or_create(temp.path().join("project.sqlite")).expect("open project");
    let source = temp.path().join("source");
    write_photo(&source.join("b.jpg"));
    write_photo(&source.join("a.jpg"));
    fs::write(source.join("notes.txt"), b"ignore me").expect("write unsupported file");

    let summary = project
        .import_folder(&source)
        .expect("import source folder");

    assert_eq!(summary.imported, 2);
    assert_eq!(summary.unsupported, 1);
    let images = project.images_by_position().expect("list images");
    assert_eq!(images[0].filename(), "a.jpg");
    assert_eq!(images[0].position, 1);
    assert_eq!(images[1].filename(), "b.jpg");
    assert_eq!(images[1].position, 2);

    project
        .set_status(images[0].id, ImageStatus::Selected)
        .expect("persist selected choice");
    let repeat = project
        .import_folder(&source)
        .expect("re-import source folder");
    assert_eq!(repeat.imported, 0);
    assert_eq!(repeat.duplicates, 2);
    let after_repeat = project.images_by_position().expect("list after reimport");
    assert_eq!(after_repeat[0].status, ImageStatus::Selected);
    assert_eq!(after_repeat[0].position, 1);

    write_photo(&source.join("c.jpg"));
    let append = project
        .import_folder(&source)
        .expect("import appended photo");
    assert_eq!(append.imported, 1);
    let after_append = project.images_by_position().expect("list after append");
    assert_eq!(after_append[2].filename(), "c.jpg");
    assert_eq!(after_append[2].position, 3);

    let next = project
        .next_unseen()
        .expect("query next unseen")
        .expect("has unseen");
    assert_eq!(next.filename(), "b.jpg");

    project
        .set_status(next.id, ImageStatus::Undecided)
        .expect("persist later choice");
    let next = project
        .next_unseen()
        .expect("query next unseen")
        .expect("has unseen");
    assert_eq!(next.filename(), "c.jpg");
}

#[test]
fn import_prefers_exif_capture_time_over_file_mtime_to_keep_camera_rolls_chronological() {
    let temp = TempDir::new().expect("create temp dir");
    let project =
        Project::open_or_create(temp.path().join("project.sqlite")).expect("open project");
    let source = temp.path().join("source");
    let mtime_only = source.join("a_mtime_only.jpg");
    let exif_photo = source.join("z_exif_photo.jpg");
    write_photo(&mtime_only);
    write_photo_with_exif_datetime_original(&exif_photo, "2010:01:02 03:04:05");
    set_mtime(&mtime_only, 1_577_836_800);
    set_mtime(&exif_photo, 1_893_456_000);

    project
        .import_folder(&source)
        .expect("import source folder");

    let images = project.images_by_position().expect("list images");
    assert_eq!(images[0].filename(), "z_exif_photo.jpg");
    assert_eq!(images[0].ordering_source, "exif");
    assert_eq!(
        images[0].ordering_timestamp.as_deref(),
        Some("2010-01-02T03:04:05Z")
    );
    assert_eq!(images[1].filename(), "a_mtime_only.jpg");
    assert_eq!(images[1].ordering_source, "mtime");
}

#[test]
fn import_uses_file_mtime_when_exif_capture_time_is_absent_to_keep_plain_images_ordered() {
    let temp = TempDir::new().expect("create temp dir");
    let project =
        Project::open_or_create(temp.path().join("project.sqlite")).expect("open project");
    let source = temp.path().join("source");
    let photo = source.join("plain.jpg");
    write_photo(&photo);
    set_mtime(&photo, 946_684_800);

    project
        .import_folder(&source)
        .expect("import source folder");

    let images = project.images_by_position().expect("list images");
    assert_eq!(images[0].filename(), "plain.jpg");
    assert_eq!(images[0].ordering_source, "mtime");
    assert_eq!(
        images[0].ordering_timestamp.as_deref(),
        Some("2000-01-01T00:00:00Z")
    );
}

#[test]
fn unseen_lookahead_query_skips_current_image_to_preload_the_next_choice() {
    let temp = TempDir::new().expect("create temp dir");
    let project =
        Project::open_or_create(temp.path().join("project.sqlite")).expect("open project");
    let source = temp.path().join("source");
    write_photo(&source.join("a.jpg"));
    write_photo(&source.join("b.jpg"));
    write_photo(&source.join("c.jpg"));
    project
        .import_folder(&source)
        .expect("import source folder");
    let images = project.images_by_position().expect("list images");
    project
        .set_status(images[1].id, ImageStatus::Rejected)
        .expect("mark middle rejected");

    let next_after_first = project
        .next_unseen_after(Some(images[0].position))
        .expect("query next unseen after current")
        .expect("has next unseen");
    assert_eq!(next_after_first.filename(), "c.jpg");
    assert!(
        project
            .next_unseen_after(Some(next_after_first.position))
            .expect("query after final unseen")
            .is_none()
    );
}

#[test]
fn later_review_query_advances_by_position_without_mixing_in_unseen_images() {
    let temp = TempDir::new().expect("create temp dir");
    let project =
        Project::open_or_create(temp.path().join("project.sqlite")).expect("open project");
    let source = temp.path().join("source");
    write_photo(&source.join("a.jpg"));
    write_photo(&source.join("b.jpg"));
    write_photo(&source.join("c.jpg"));
    project
        .import_folder(&source)
        .expect("import source folder");
    let images = project.images_by_position().expect("list images");
    project
        .set_status(images[0].id, ImageStatus::Undecided)
        .expect("mark first later");
    project
        .set_status(images[2].id, ImageStatus::Undecided)
        .expect("mark third later");

    let first_later = project
        .next_undecided_after(None)
        .expect("query first later")
        .expect("has first later");
    assert_eq!(first_later.filename(), "a.jpg");
    let second_later = project
        .next_undecided_after(Some(first_later.position))
        .expect("query second later")
        .expect("has second later");
    assert_eq!(second_later.filename(), "c.jpg");
    assert!(
        project
            .next_undecided_after(Some(second_later.position))
            .expect("query after last later")
            .is_none()
    );
}

#[test]
fn export_copies_only_selected_images_with_unique_names_and_manifest() {
    let temp = TempDir::new().expect("create temp dir");
    let project =
        Project::open_or_create(temp.path().join("project.sqlite")).expect("open project");
    let source = temp.path().join("source");
    write_photo(&source.join("one/IMG_0001.jpg"));
    write_photo(&source.join("two/IMG_0001.jpg"));
    write_photo(&source.join("three/IMG_0002.png"));
    project
        .import_folder(&source)
        .expect("import source folder");
    let images = project.images_by_position().expect("list images");

    for image in &images {
        let status = if image.filename() == "IMG_0001.jpg" {
            ImageStatus::Selected
        } else {
            ImageStatus::Rejected
        };
        project
            .set_status(image.id, status)
            .expect("persist export test status");
    }

    let export_root = temp.path().join("exports");
    let summary = project
        .export_selected(&export_root)
        .expect("export selected images");

    assert_eq!(summary.copied, 2);
    assert_eq!(summary.failed.len(), 0);
    assert!(summary.export_dir.exists());
    assert!(summary.export_dir.join("000001_IMG_0001.jpg").exists());
    assert!(summary.export_dir.join("000002_IMG_0001.jpg").exists());
    assert!(!summary.export_dir.join("000003_IMG_0002.png").exists());

    let manifest =
        fs::read_to_string(summary.export_dir.join("manifest.csv")).expect("read manifest");
    assert!(manifest.contains("exported_filename,original_path,status,position"));
    assert!(manifest.contains("000001_IMG_0001.jpg"));
    assert!(manifest.contains("000002_IMG_0001.jpg"));
    assert!(manifest.contains("selected"));
}

#[test]
fn export_continues_after_a_selected_file_cannot_be_copied() {
    let temp = TempDir::new().expect("create temp dir");
    let project =
        Project::open_or_create(temp.path().join("project.sqlite")).expect("open project");
    let source = temp.path().join("source");
    write_photo(&source.join("a.jpg"));
    write_photo(&source.join("b.jpg"));
    project
        .import_folder(&source)
        .expect("import source folder");
    let images = project.images_by_position().expect("list images");
    for image in &images {
        project
            .set_status(image.id, ImageStatus::Selected)
            .expect("select image");
    }
    fs::remove_file(&images[0].path).expect("remove one source file to simulate export failure");

    let summary = project
        .export_selected(temp.path().join("exports"))
        .expect("export selected images");

    assert_eq!(summary.copied, 1);
    assert_eq!(summary.failed.len(), 1);
    assert!(summary.export_dir.join("000002_b.jpg").exists());
}
