use crate::locale::tr;
use crate::tab::icon_button;
use gtk::{gdk, gio, glib, graphene, prelude::*};
use image::{ImageDecoder, ImageFormat, ImageReader};
use mosaic_shell_tools::diagnostics;
use std::{
    cell::{Cell, RefCell},
    fs::File,
    io::BufReader,
    path::PathBuf,
    rc::Rc,
};

const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_SIDE: u32 = 12_000;
const MAX_PIXELS: u64 = 24_000_000;
const ZOOM_PERCENT: [u32; 6] = [25, 50, 75, 100, 150, 200];

#[derive(Clone)]
pub struct Photo {
    path: PathBuf,
    format: ImageFormat,
}

impl Photo {
    pub fn from_file(file: &gio::File) -> Option<Self> {
        let path = file.path()?;
        let format = match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
            "jpg" | "jpeg" => ImageFormat::Jpeg,
            "png" => ImageFormat::Png,
            "webp" => ImageFormat::WebP,
            _ => return None,
        };
        Some(Self { path, format })
    }
}

enum DecodeError {
    Read,
    Format,
    Limit,
}

impl From<image::ImageError> for DecodeError {
    fn from(error: image::ImageError) -> Self {
        match error {
            image::ImageError::Limits(_) => Self::Limit,
            image::ImageError::IoError(_) => Self::Read,
            _ => Self::Format,
        }
    }
}

fn decode(photo: &Photo) -> Result<image::RgbaImage, DecodeError> {
    // Avoid opening special files such as pipes. Check the open handle too in case it changed.
    let check = |metadata: std::fs::Metadata| {
        if !metadata.is_file() {
            return Err(DecodeError::Read);
        }
        if metadata.len() > MAX_FILE_BYTES {
            return Err(DecodeError::Limit);
        }
        Ok(())
    };
    check(std::fs::metadata(&photo.path).map_err(|_| DecodeError::Read)?)?;
    let file = File::open(&photo.path).map_err(|_| DecodeError::Read)?;
    check(file.metadata().map_err(|_| DecodeError::Read)?)?;

    let mut reader = ImageReader::with_format(BufReader::new(file), photo.format);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    limits.max_alloc = Some(192 * 1024 * 1024);
    reader.limits(limits);
    let mut decoder = reader.into_decoder()?;
    let (width, height) = decoder.dimensions();
    if u64::from(width) * u64::from(height) > MAX_PIXELS {
        return Err(DecodeError::Limit);
    }
    let orientation = decoder
        .orientation()
        .unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut image = image::DynamicImage::from_decoder(decoder)?;
    image.apply_orientation(orientation);
    Ok(image.into_rgba8())
}

pub fn show(parent: &gtk::Window, mut photos: Vec<Photo>, selected: Photo) {
    let index = photos.iter().position(|photo| photo.path == selected.path);
    let index = index.unwrap_or_else(|| {
        photos.clear();
        photos.push(selected);
        0
    });

    let window = gtk::Window::builder()
        .transient_for(parent)
        .destroy_with_parent(true)
        .modal(true)
        .default_width(880)
        .default_height(600)
        .build();
    window.set_application(parent.application().as_ref());

    let previous = icon_button("go-previous-symbolic", tr("Предыдущее фото (←)"));
    let next = icon_button("go-next-symbolic", tr("Следующее фото (→)"));
    let zoom = gtk::DropDown::from_strings(&[
        tr("По окну"),
        "25 %",
        "50 %",
        "75 %",
        "100 %",
        "150 %",
        "200 %",
    ]);
    zoom.set_tooltip_text(Some(tr("Масштаб (+, −, 0)")));
    zoom.set_sensitive(false);
    let count = gtk::Label::new(None);
    let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    toolbar.set_halign(gtk::Align::Center);
    toolbar.set_margin_top(6);
    toolbar.set_margin_bottom(6);
    toolbar.append(&previous);
    toolbar.append(&count);
    toolbar.append(&next);
    toolbar.append(&zoom);

    let picture = gtk::Picture::new();
    picture.set_content_fit(gtk::ContentFit::Contain);
    let scroll = gtk::ScrolledWindow::builder()
        .child(&picture)
        .vexpand(true)
        .build();
    let status = gtk::Label::new(None);
    status.set_wrap(true);
    status.set_margin_start(20);
    status.set_margin_end(20);
    let body = gtk::Stack::new();
    body.set_vexpand(true);
    body.add_child(&scroll);
    body.add_child(&status);

    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.append(&toolbar);
    content.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    content.append(&body);
    window.set_child(Some(&content));

    let viewer = Rc::new(Viewer {
        window: window.downgrade(),
        photos,
        index: Cell::new(index),
        loading: Cell::new(false),
        texture: RefCell::new(None),
        picture,
        scroll,
        status,
        body,
        previous,
        next,
        count,
        zoom,
    });
    let keep_alive = viewer.clone();
    window.connect_close_request(move |_| {
        let _ = &keep_alive;
        glib::Propagation::Proceed
    });

    for (button, offset) in [(&viewer.previous, -1), (&viewer.next, 1)] {
        let weak = Rc::downgrade(&viewer);
        button.connect_clicked(move |_| {
            if let Some(viewer) = weak.upgrade() {
                viewer.step(offset);
            }
        });
    }
    let weak = Rc::downgrade(&viewer);
    viewer.zoom.connect_selected_notify(move |_| {
        if let Some(viewer) = weak.upgrade() {
            viewer.update_zoom();
        }
    });
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    let weak = Rc::downgrade(&viewer);
    keys.connect_key_pressed(move |_, key, _, _| {
        let Some(viewer) = weak.upgrade() else {
            return glib::Propagation::Proceed;
        };
        match key {
            gdk::Key::Escape => {
                if let Some(window) = viewer.window.upgrade() {
                    window.close();
                }
            }
            gdk::Key::Left => viewer.step(-1),
            gdk::Key::Right => viewer.step(1),
            gdk::Key::plus | gdk::Key::equal | gdk::Key::KP_Add => viewer.step_zoom(1),
            gdk::Key::minus | gdk::Key::KP_Subtract => viewer.step_zoom(-1),
            gdk::Key::_0 | gdk::Key::KP_0 => viewer.zoom.set_selected(0),
            _ => return glib::Propagation::Proceed,
        }
        glib::Propagation::Stop
    });
    window.add_controller(keys);

    viewer.load();
    window.present();
}

struct Viewer {
    window: glib::WeakRef<gtk::Window>,
    photos: Vec<Photo>,
    index: Cell<usize>,
    loading: Cell<bool>,
    texture: RefCell<Option<gdk::Texture>>,
    picture: gtk::Picture,
    scroll: gtk::ScrolledWindow,
    status: gtk::Label,
    body: gtk::Stack,
    previous: gtk::Button,
    next: gtk::Button,
    count: gtk::Label,
    zoom: gtk::DropDown,
}

impl Viewer {
    fn step(self: &Rc<Self>, offset: isize) {
        if let Some(index) = self.index.get().checked_add_signed(offset)
            && index < self.photos.len()
        {
            self.index.set(index);
            self.load();
        }
    }

    fn load(self: &Rc<Self>) {
        let index = self.index.get();
        let path = &self.photos[index].path;
        if let Some(window) = self.window.upgrade() {
            window.set_title(Some(
                &tr("Фото · {name}").replace("{name}", &glib::filename_display_basename(path)),
            ));
        }
        self.count
            .set_text(&format!("{} / {}", index + 1, self.photos.len()));
        self.previous.set_sensitive(index > 0);
        self.next.set_sensitive(index + 1 < self.photos.len());
        self.picture.set_paintable(gdk::Paintable::NONE);
        self.texture.take();
        self.zoom.set_sensitive(false);
        self.status.set_text(tr("Загрузка…"));
        self.body.set_visible_child(&self.status);

        // Decode only one photo at a time. If navigation outruns decoding, load the latest choice.
        if self.loading.replace(true) {
            return;
        }
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            loop {
                let (index, photo) = {
                    let Some(viewer) = weak.upgrade() else {
                        return;
                    };
                    let index = viewer.index.get();
                    (index, viewer.photos[index].clone())
                };
                let result = gio::spawn_blocking(move || decode(&photo)).await;
                let Some(viewer) = weak.upgrade() else {
                    return;
                };
                if viewer.index.get() != index {
                    continue;
                }
                viewer.loading.set(false);
                viewer.render(result.unwrap_or(Err(DecodeError::Read)));
                return;
            }
        });
    }

    fn render(&self, result: Result<image::RgbaImage, DecodeError>) {
        let pixels = match result {
            Ok(pixels) => pixels,
            Err(error) => {
                let (event, message) = match error {
                    DecodeError::Read => {
                        ("photo_read_failed", tr("Не удалось прочитать изображение"))
                    }
                    DecodeError::Format => (
                        "photo_decode_failed",
                        tr("Изображение повреждено или его формат не поддерживается"),
                    ),
                    DecodeError::Limit => (
                        "photo_limit_exceeded",
                        tr("Изображение превышает допустимый размер"),
                    ),
                };
                diagnostics::event("files", event, 0);
                self.status.set_text(message);
                return;
            }
        };
        let (width, height) = pixels.dimensions();
        let texture = gdk::MemoryTexture::new(
            width as i32,
            height as i32,
            gdk::MemoryFormat::R8g8b8a8,
            &glib::Bytes::from_owned(pixels.into_raw()),
            width as usize * 4,
        );
        self.texture.replace(Some(texture.upcast()));
        self.zoom.set_sensitive(true);
        self.update_zoom();
        self.scroll.hadjustment().set_value(0.0);
        self.scroll.vadjustment().set_value(0.0);
        self.body.set_visible_child(&self.scroll);
    }

    fn step_zoom(&self, offset: i32) {
        if self.texture.borrow().is_none() {
            return;
        }
        let current = self.zoom.selected() as i32;
        let index = if current == 0 {
            if offset > 0 { 4 } else { 2 }
        } else {
            (current + offset).clamp(0, ZOOM_PERCENT.len() as i32)
        };
        self.zoom.set_selected(index as u32);
    }

    fn update_zoom(&self) {
        let texture = self.texture.borrow();
        let Some(texture) = texture.as_ref() else {
            return;
        };
        let index = self.zoom.selected() as usize;
        if index == 0 {
            self.scroll
                .set_policy(gtk::PolicyType::Never, gtk::PolicyType::Never);
            self.picture.set_can_shrink(true);
            self.picture.set_halign(gtk::Align::Fill);
            self.picture.set_valign(gtk::Align::Fill);
            self.picture.set_paintable(Some(texture));
        } else if let Some(percent) = ZOOM_PERCENT.get(index - 1) {
            self.scroll
                .set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Automatic);
            self.picture.set_can_shrink(false);
            self.picture.set_halign(gtk::Align::Center);
            self.picture.set_valign(gtk::Align::Center);
            let width = ((texture.width() as f32 * *percent as f32 / 100.0).round()).max(1.0);
            let height = ((texture.height() as f32 * *percent as f32 / 100.0).round()).max(1.0);
            let snapshot = gtk::Snapshot::new();
            snapshot.append_texture(texture, &graphene::Rect::new(0.0, 0.0, width, height));
            let paintable = snapshot.to_paintable(Some(&graphene::Size::new(width, height)));
            self.picture.set_paintable(paintable.as_ref());
        }
    }
}
