use crate::{
    db::DB,
    image_editing::EditState,
    scrubber::Scrubber,
    settings::PersistentSettings,
    utils::{ExtendedImageInfo, Frame, Player},
};
use image::RgbaImage;
use nalgebra::Vector2;
use notan::{egui::epaint::ahash::HashMap, prelude::Texture, AppState};
use std::{
    default::Default,
    path::PathBuf,
    sync::mpsc::{self, Receiver, Sender},
    time::Instant,
};

#[derive(Debug, Clone)]
pub struct ImageGeometry {
    /// The scale of the displayed image
    pub scale: f32,
    /// Image offset on canvas
    pub offset: Vector2<f32>,
}

#[derive(Debug, Clone)]
pub enum Message {
    Info(String),
    Warning(String),
    Error(String),
    LoadError(String),
}

impl Message {
    pub fn info(m: &str) -> Self {
        Self::Info(m.into())
    }
    pub fn warn(m: &str) -> Self {
        Self::Warning(m.into())
    }
    pub fn err(m: &str) -> Self {
        Self::Error(m.into())
    }
}

/// The state of the application
#[derive(Debug, AppState)]
pub struct OculanteState {
    pub image_geometry: ImageGeometry,
    pub compare_list: HashMap<PathBuf, ImageGeometry>,
    pub drag_enabled: bool,
    pub reset_image: bool,
    pub message: Option<Message>,
    /// Is the image fully loaded?
    pub is_loaded: bool,
    pub window_size: Vector2<f32>,
    pub cursor: Vector2<f32>,
    pub cursor_relative: Vector2<f32>,
    pub image_dimension: (u32, u32),
    pub sampled_color: [f32; 4],
    pub mouse_delta: Vector2<f32>,
    pub texture_channel: (Sender<Frame>, Receiver<Frame>),
    pub message_channel: (Sender<Message>, Receiver<Message>),
    /// Channel to load images from
    pub load_channel: (Sender<PathBuf>, Receiver<PathBuf>),
    pub extended_info_channel: (Sender<ExtendedImageInfo>, Receiver<ExtendedImageInfo>),
    pub extended_info_loading: bool,
    /// The Player, responsible for loading and sending Frames
    pub player: Player,
    pub current_texture: Option<Texture>,
    pub current_path: Option<PathBuf>,
    pub current_image: Option<RgbaImage>,
    pub settings_enabled: bool,
    pub image_info: Option<ExtendedImageInfo>,
    pub tiling: usize,
    pub mouse_grab: bool,
    pub key_grab: bool,
    pub edit_state: EditState,
    pub pointer_over_ui: bool,
    /// Things that perisist between launches
    pub persistent_settings: PersistentSettings,
    pub always_on_top: bool,
    pub network_mode: bool,
    /// how long the toast message appears
    pub toast_cooldown: Instant,
    /// data to transform image once fullscreen is entered/left
    pub fullscreen_offset: Option<(i32, i32)>,
    /// List of images to cycle through. Usually the current dir or dropped files
    pub scrubber: Scrubber,
    pub checker_texture: Option<Texture>,
    pub redraw: bool,
    pub folder_selected: Option<PathBuf>,
    pub toggle_slideshow: bool,
    pub slideshow_time: Instant,
    pub current_image_is_favourite: bool,
    pub db: Option<DB>,
    pub show_metadata_tooltip: bool,
    pub first_start: bool,
}

impl OculanteState {
    pub fn send_message(&self, msg: &str) {
        _ = self.message_channel.0.send(Message::info(msg));
    }

    pub fn send_message_err(&self, msg: &str) {
        _ = self.message_channel.0.send(Message::err(msg));
    }

    pub fn reload_image(&mut self) {
        match self.scrubber.set(self.scrubber.index) {
            Ok(img_path) => {
                self.is_loaded = false;
                self.current_path = Some(img_path.clone());
                self.player.load(img_path.as_path(), self.message_channel.0.clone());
            },
            Err(_) => {
                self.reset();
                self.send_message_err("No images");
            }
        }
    }

    pub fn cursor_within_image(& self) -> bool {
        let img_dims_scaled = (
            self.image_dimension.0 as f32 * self.image_geometry.scale,
            self.image_dimension.1 as f32 * self.image_geometry.scale,
        );
        let img_x = (
            self.image_geometry.offset[0],
            self.image_geometry.offset[0] + img_dims_scaled.0,
        );
        let img_y = (
            self.image_geometry.offset[1],
            self.image_geometry.offset[1] + img_dims_scaled.1,
        );

        if img_x.0 <= self.cursor[0]
            && self.cursor[0] <= img_x.1
            && img_y.0 <= self.cursor[1]
            && self.cursor[1] <= img_y.1
        {
            return true;
        }
        false
    }

    fn reset(&mut self) {
        *self = OculanteState::default();
    }
}

impl Default for OculanteState {
    fn default() -> OculanteState {
        let tx_channel = mpsc::channel();
        OculanteState {
            image_geometry: ImageGeometry {
                scale: 1.0,
                offset: Default::default(),
            },
            compare_list: Default::default(),
            drag_enabled: Default::default(),
            reset_image: Default::default(),
            message: Default::default(),
            is_loaded: Default::default(),
            cursor: Default::default(),
            cursor_relative: Default::default(),
            image_dimension: (0, 0),
            sampled_color: [0., 0., 0., 0.],
            player: Player::new(tx_channel.0.clone(), 20, 16384),
            texture_channel: tx_channel,
            message_channel: mpsc::channel(),
            load_channel: mpsc::channel(),
            extended_info_channel: mpsc::channel(),
            extended_info_loading: Default::default(),
            mouse_delta: Default::default(),
            current_texture: Default::default(),
            current_image: Default::default(),
            current_path: Default::default(),
            settings_enabled: Default::default(),
            image_info: Default::default(),
            tiling: 1,
            mouse_grab: Default::default(),
            key_grab: Default::default(),
            edit_state: Default::default(),
            pointer_over_ui: Default::default(),
            persistent_settings: Default::default(),
            always_on_top: Default::default(),
            network_mode: Default::default(),
            window_size: Default::default(),
            toast_cooldown: Instant::now(),
            fullscreen_offset: Default::default(),
            scrubber: Default::default(),
            checker_texture: Default::default(),
            redraw: Default::default(),
            folder_selected: Default::default(),
            toggle_slideshow: false,
            slideshow_time: Instant::now(),
            current_image_is_favourite: Default::default(),
            db: None,
            show_metadata_tooltip: false,
            first_start: true,
        }
    }
}
