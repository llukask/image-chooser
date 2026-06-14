use std::path::PathBuf;

use iced::{
    ContentFit, Element, Length, Subscription, Task, Theme, keyboard,
    widget::{button, column, container, image as iced_image, row, text},
};
use image_chooser::{ImageChoice, ImageStatus, Project, StatusCounts, default_project_path};

pub fn run_gui(project_path: Option<PathBuf>) -> iced::Result {
    iced::application(
        move || ImageChooserApp::boot(project_path.clone()),
        ImageChooserApp::update,
        ImageChooserApp::view,
    )
    .subscription(ImageChooserApp::subscription)
    .title("Image Chooser")
    .theme(app_theme)
    .window(iced::window::Settings {
        maximized: true,
        ..iced::window::Settings::default()
    })
    .run()
}

#[derive(Debug)]
struct ImageChooserApp {
    project: Option<Project>,
    current: Option<ImageChoice>,
    counts: StatusCounts,
    queue: SelectionQueue,
    review_after_position: Option<i64>,
    load_state: LoadState,
    undo_stack: Vec<UndoAction>,
    status: String,
    zoom: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SelectionQueue {
    MainUnseen,
    ReviewLater,
}

#[derive(Debug, Clone)]
enum LoadState {
    Idle,
    Loading,
    Loaded { image: LoadedImage },
    Failed { message: String },
}

#[derive(Debug, Clone)]
struct LoadedImage {
    handle: iced_image::Handle,
    width: u32,
    height: u32,
}

#[derive(Debug, Clone)]
struct ImageLoadFinished {
    image_id: i64,
    result: Result<LoadedImage, String>,
}

#[derive(Debug, Clone)]
struct UndoAction {
    image_id: i64,
    previous_status: ImageStatus,
    queue: SelectionQueue,
    review_after_position: Option<i64>,
}

#[derive(Debug, Clone)]
enum Message {
    Choose(ChoiceAction),
    Undo,
    StartReviewLater,
    ExitReviewLater,
    ImageLoaded(ImageLoadFinished),
    CloseZoom,
    KeyboardShortcut(KeyboardShortcut),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChoiceAction {
    Select,
    Reject,
    Later,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KeyboardShortcut {
    Select,
    Reject,
    Later,
    Undo,
    Escape,
}

fn app_theme(_: &ImageChooserApp) -> Theme {
    Theme::Light
}

impl ImageChooserApp {
    fn boot(project_path: Option<PathBuf>) -> (Self, Task<Message>) {
        let mut app = Self {
            project: None,
            current: None,
            counts: StatusCounts::default(),
            queue: SelectionQueue::MainUnseen,
            review_after_position: None,
            load_state: LoadState::Idle,
            undo_stack: Vec::new(),
            status: String::new(),
            zoom: false,
        };

        let resolved_project_path = match project_path {
            Some(path) => Some(path),
            None => match default_project_path() {
                Ok(path) if path.exists() => Some(path),
                Ok(path) => {
                    app.status = format!(
                        "Noch kein Projekt geladen. Standardpfad wäre: {}",
                        path.display()
                    );
                    None
                }
                Err(error) => {
                    app.status =
                        format!("Standard-Projektpfad konnte nicht bestimmt werden: {error}");
                    None
                }
            },
        };

        let task = match resolved_project_path {
            Some(path) => match Project::open_or_create(&path) {
                Ok(project) => {
                    app.status = format!("Projekt geladen: {}", path.display());
                    app.project = Some(project);
                    app.refresh_counts();
                    app.load_next_image()
                }
                Err(error) => {
                    app.status = format!("Projekt konnte nicht geöffnet werden: {error}");
                    Task::none()
                }
            },
            None => Task::none(),
        };

        (app, task)
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Choose(action) => self.apply_choice(action),
            Message::Undo => self.undo_last_choice(),
            Message::StartReviewLater => {
                self.queue = SelectionQueue::ReviewLater;
                self.review_after_position = None;
                self.zoom = false;
                self.status = "Später-Entscheidungen ansehen.".to_owned();
                self.load_next_image()
            }
            Message::ExitReviewLater => {
                self.queue = SelectionQueue::MainUnseen;
                self.review_after_position = None;
                self.zoom = false;
                self.status = "Zurück zur normalen Auswahl.".to_owned();
                self.load_next_image()
            }
            Message::ImageLoaded(finished) => {
                self.finish_image_load(finished);
                Task::none()
            }
            Message::CloseZoom => {
                self.zoom = false;
                Task::none()
            }
            Message::KeyboardShortcut(shortcut) => self.apply_keyboard_shortcut(shortcut),
        }
    }

    fn subscription(&self) -> Subscription<Message> {
        keyboard::listen().filter_map(|event| {
            let keyboard::Event::KeyPressed {
                modified_key,
                repeat: false,
                ..
            } = event
            else {
                return None;
            };

            match modified_key.as_ref() {
                keyboard::Key::Character("y") | keyboard::Key::Character("Y") => {
                    Some(Message::KeyboardShortcut(KeyboardShortcut::Select))
                }
                keyboard::Key::Character("n") | keyboard::Key::Character("N") => {
                    Some(Message::KeyboardShortcut(KeyboardShortcut::Reject))
                }
                keyboard::Key::Character("l") | keyboard::Key::Character("L") => {
                    Some(Message::KeyboardShortcut(KeyboardShortcut::Later))
                }
                keyboard::Key::Character("u") | keyboard::Key::Character("U") => {
                    Some(Message::KeyboardShortcut(KeyboardShortcut::Undo))
                }
                keyboard::Key::Named(keyboard::key::Named::Escape) => {
                    Some(Message::KeyboardShortcut(KeyboardShortcut::Escape))
                }
                _ => None,
            }
        })
    }

    fn view(&self) -> Element<'_, Message> {
        if self.zoom {
            return self.view_zoom();
        }

        if self.project.is_none() {
            return self.view_setup_placeholder();
        }

        if self.current.is_none() {
            return self.view_completion();
        }

        self.view_selection()
    }

    fn view_setup_placeholder(&self) -> Element<'_, Message> {
        let content = column![
            text("Fotoauswahl").size(56),
            text("Projekt einrichten").size(34),
            text(&self.status).size(24),
            text("Bitte zuerst im Terminal ausführen:").size(24),
            text("image-chooser init").size(28),
            text("image-chooser import /pfad/zu/fotos").size(28),
            text("image-chooser gui").size(28),
        ]
        .spacing(24)
        .padding(40);

        centered(content).into()
    }

    fn view_completion(&self) -> Element<'_, Message> {
        let mut content = column![
            text(match self.queue {
                SelectionQueue::MainUnseen => "Fertig: keine neuen Bilder mehr",
                SelectionQueue::ReviewLater => "Fertig: keine Später-Bilder mehr",
            })
            .size(50),
            text(counts_text(self.counts)).size(28),
            text(&self.status).size(24),
        ]
        .spacing(24)
        .padding(40);

        if self.queue == SelectionQueue::MainUnseen && self.counts.undecided > 0 {
            content = content.push(
                button(text("Später entscheiden ansehen").size(34))
                    .padding(24)
                    .on_press(Message::StartReviewLater),
            );
        }

        if self.queue == SelectionQueue::ReviewLater {
            content = content.push(
                button(text("Zur normalen Auswahl zurück").size(30))
                    .padding(20)
                    .on_press(Message::ExitReviewLater),
            );
        }

        centered(content).into()
    }

    fn view_selection(&self) -> Element<'_, Message> {
        let current = self.current.as_ref().expect("current image exists");
        let header = row![
            text(current.filename()).size(34),
            text(queue_text(self.queue)).size(24),
            text(counts_text(self.counts)).size(24),
        ]
        .spacing(30);

        let image_area = self.view_image_area(false);

        let button_width = Length::Fixed(220.0);
        let undo_button = if self.undo_stack.is_empty() {
            button(text("Rückgängig").size(32).width(Length::Fill).center())
                .padding(22)
                .width(button_width)
        } else {
            button(text("Rückgängig").size(32).width(Length::Fill).center())
                .padding(22)
                .width(button_width)
                .on_press(Message::Undo)
        };

        let controls = row![
            button(text("Ja").size(32).width(Length::Fill).center())
                .padding(22)
                .width(button_width)
                .on_press(Message::Choose(ChoiceAction::Select)),
            button(text("Nein").size(32).width(Length::Fill).center())
                .padding(22)
                .width(button_width)
                .on_press(Message::Choose(ChoiceAction::Reject)),
            button(text("Später").size(32).width(Length::Fill).center())
                .padding(22)
                .width(button_width)
                .on_press(Message::Choose(ChoiceAction::Later)),
            undo_button,
        ]
        .spacing(18);

        let centered_controls = container(controls)
            .width(Length::Fill)
            .center_x(Length::Fill);

        let shortcuts =
            text("Tastenkürzel: Y = Ja · N = Nein · L = Später · U = Rückgängig").size(20);

        let content = column![
            header,
            image_area,
            centered_controls,
            shortcuts,
            text(&self.status).size(22)
        ]
        .spacing(18)
        .padding(24)
        .width(Length::Fill)
        .height(Length::Fill);

        container(content)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    fn view_zoom(&self) -> Element<'_, Message> {
        let filename = self
            .current
            .as_ref()
            .map(ImageChoice::filename)
            .unwrap_or_else(|| "Kein Bild".to_owned());

        let content = column![
            row![
                text(filename).size(30),
                button(text("Zurück (Esc)").size(30))
                    .padding(18)
                    .on_press(Message::CloseZoom),
            ]
            .spacing(30),
            self.view_image_area(true),
        ]
        .spacing(18)
        .padding(20)
        .width(Length::Fill)
        .height(Length::Fill);

        container(content)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    fn view_image_area(&self, _zoom: bool) -> Element<'_, Message> {
        let body: Element<'_, Message> = match &self.load_state {
            LoadState::Idle => text("Kein Bild geladen").size(34).into(),
            LoadState::Loading => text("Bild wird geladen …").size(38).into(),
            LoadState::Loaded { image } => column![
                iced_image(image.handle.clone())
                    .content_fit(ContentFit::Contain)
                    .width(Length::Fill)
                    .height(Length::Fill),
                text(format!("{} × {}", image.width, image.height)).size(18),
            ]
            .spacing(8)
            .width(Length::Fill)
            .height(Length::Fill)
            .into(),
            LoadState::Failed { message } => column![
                text("Dieses Bild kann gerade nicht angezeigt werden.").size(36),
                text("Sichere Standard-Aktion: Später entscheiden.").size(28),
                text(message).size(20),
                button(text("Später").size(34))
                    .padding(24)
                    .on_press(Message::Choose(ChoiceAction::Later)),
            ]
            .spacing(20)
            .padding(30)
            .into(),
        };

        container(body)
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .into()
    }

    fn apply_choice(&mut self, action: ChoiceAction) -> Task<Message> {
        let Some(current) = self.current.clone() else {
            return Task::none();
        };
        let Some(project) = &self.project else {
            return Task::none();
        };

        let new_status = match action {
            ChoiceAction::Select => ImageStatus::Selected,
            ChoiceAction::Reject => ImageStatus::Rejected,
            ChoiceAction::Later => ImageStatus::Undecided,
        };

        let undo = UndoAction {
            image_id: current.id,
            previous_status: current.status,
            queue: self.queue,
            review_after_position: self.review_after_position,
        };

        match project.set_status(current.id, new_status) {
            Ok(()) => {
                self.undo_stack.push(undo);
                self.zoom = false;
                if self.queue == SelectionQueue::ReviewLater {
                    self.review_after_position = Some(current.position);
                }
                self.status = choice_status_text(action).to_owned();
                self.refresh_counts();
                self.load_next_image()
            }
            Err(error) => {
                self.status = format!("Auswahl konnte nicht gespeichert werden: {error}");
                Task::none()
            }
        }
    }

    fn undo_last_choice(&mut self) -> Task<Message> {
        let Some(undo) = self.undo_stack.pop() else {
            return Task::none();
        };
        let restore_result = self
            .project
            .as_ref()
            .map(|project| project.set_status(undo.image_id, undo.previous_status));
        match restore_result {
            Some(Ok(())) => {}
            Some(Err(error)) => {
                self.status = format!("Rückgängig konnte nicht gespeichert werden: {error}");
                return Task::none();
            }
            None => return Task::none(),
        }

        self.queue = undo.queue;
        self.review_after_position = undo.review_after_position;
        self.zoom = false;
        self.refresh_counts();

        let image_result = self
            .project
            .as_ref()
            .map(|project| project.image_by_id(undo.image_id));

        match image_result {
            Some(Ok(Some(image))) => {
                self.current = Some(image);
                self.status = "Rückgängig gemacht.".to_owned();
                self.load_current_image()
            }
            Some(Ok(None)) => {
                self.status = "Bild für Rückgängig wurde nicht gefunden.".to_owned();
                self.load_next_image()
            }
            Some(Err(error)) => {
                self.status = format!("Bild für Rückgängig konnte nicht geladen werden: {error}");
                Task::none()
            }
            None => Task::none(),
        }
    }

    fn apply_keyboard_shortcut(&mut self, shortcut: KeyboardShortcut) -> Task<Message> {
        match shortcut {
            KeyboardShortcut::Select if !self.zoom => self.apply_choice(ChoiceAction::Select),
            KeyboardShortcut::Reject if !self.zoom => self.apply_choice(ChoiceAction::Reject),
            KeyboardShortcut::Later if !self.zoom => self.apply_choice(ChoiceAction::Later),
            KeyboardShortcut::Undo if !self.zoom => self.undo_last_choice(),
            KeyboardShortcut::Escape => {
                self.zoom = false;
                Task::none()
            }
            _ => Task::none(),
        }
    }

    fn load_next_image(&mut self) -> Task<Message> {
        let Some(project) = &self.project else {
            return Task::none();
        };

        let next = match self.queue {
            SelectionQueue::MainUnseen => project.next_unseen(),
            SelectionQueue::ReviewLater => project.next_undecided_after(self.review_after_position),
        };

        match next {
            Ok(Some(image)) => {
                self.current = Some(image);
                self.load_current_image()
            }
            Ok(None) => {
                self.current = None;
                self.load_state = LoadState::Idle;
                self.refresh_counts();
                Task::none()
            }
            Err(error) => {
                self.status = format!("Nächstes Bild konnte nicht geladen werden: {error}");
                Task::none()
            }
        }
    }

    fn load_current_image(&mut self) -> Task<Message> {
        let Some(current) = &self.current else {
            self.load_state = LoadState::Idle;
            return Task::none();
        };

        let image_id = current.id;
        let path = current.path.clone();
        self.load_state = LoadState::Loading;

        Task::perform(
            async move {
                ImageLoadFinished {
                    image_id,
                    result: load_image_for_display(path),
                }
            },
            Message::ImageLoaded,
        )
    }

    fn finish_image_load(&mut self, finished: ImageLoadFinished) {
        if !matches!(
            self.current.as_ref(),
            Some(current) if current.id == finished.image_id
        ) {
            return;
        }

        match finished.result {
            Ok(image) => {
                if let Some(project) = &self.project
                    && let Err(error) = project.clear_last_error(finished.image_id)
                {
                    self.status =
                        format!("Bild geladen, Fehlerstatus konnte nicht gelöscht werden: {error}");
                }
                self.load_state = LoadState::Loaded { image };
            }
            Err(message) => {
                if let Some(project) = &self.project
                    && let Err(error) = project.store_last_error(finished.image_id, &message)
                {
                    self.status = format!("Bildfehler konnte nicht gespeichert werden: {error}");
                }
                self.load_state = LoadState::Failed { message };
            }
        }
    }

    fn refresh_counts(&mut self) {
        if let Some(project) = &self.project {
            match project.status_counts() {
                Ok(counts) => self.counts = counts,
                Err(error) => self.status = format!("Status konnte nicht gelesen werden: {error}"),
            }
        }
    }
}

fn load_image_for_display(path: PathBuf) -> Result<LoadedImage, String> {
    use ::image::{DynamicImage, ImageDecoder, ImageReader};

    let reader = ImageReader::open(&path).map_err(|error| error.to_string())?;
    let reader = reader
        .with_guessed_format()
        .map_err(|error| error.to_string())?;
    let mut decoder = reader.into_decoder().map_err(|error| error.to_string())?;
    let orientation = decoder.orientation().map_err(|error| error.to_string())?;
    let mut image = DynamicImage::from_decoder(decoder).map_err(|error| error.to_string())?;
    image.apply_orientation(orientation);

    let rgba = image.to_rgba8();
    let (width, height) = rgba.dimensions();

    Ok(LoadedImage {
        handle: iced_image::Handle::from_rgba(width, height, rgba.into_raw()),
        width,
        height,
    })
}

fn centered<'a>(content: impl Into<Element<'a, Message>>) -> container::Container<'a, Message> {
    container(content)
        .width(Length::Fill)
        .height(Length::Fill)
        .center_x(Length::Fill)
        .center_y(Length::Fill)
}

fn counts_text(counts: StatusCounts) -> String {
    format!(
        "Neu: {} · Ausgewählt: {} · Später: {} · Abgelehnt: {}",
        counts.unseen, counts.selected, counts.undecided, counts.rejected
    )
}

fn queue_text(queue: SelectionQueue) -> &'static str {
    match queue {
        SelectionQueue::MainUnseen => "Neue Bilder",
        SelectionQueue::ReviewLater => "Später ansehen",
    }
}

fn choice_status_text(action: ChoiceAction) -> &'static str {
    match action {
        ChoiceAction::Select => "Gespeichert: Ja, drucken.",
        ChoiceAction::Reject => "Gespeichert: Nein, nicht drucken.",
        ChoiceAction::Later => "Gespeichert: Später entscheiden.",
    }
}
