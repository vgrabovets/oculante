#![windows_subsystem = "windows"]

use clap::Arg;
use clap::Command;
use clipboard::{ClipboardContext, ClipboardProvider};
use log::{debug, error, info, warn};
use nalgebra::Vector2;
use notan::app::Event;
use notan::draw::*;
use notan::egui::{self, *};
use notan::prelude::*;
use round::round;
use shortcuts::key_pressed;
use std::collections::HashSet;
use std::ffi::OsStr;
use std::fs::{self, File};
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};
pub mod cache;
pub mod scrubber;
pub mod settings;
pub mod shortcuts;
#[cfg(feature = "turbo")]
use crate::image_editing::lossless_tx;
use crate::scrubber::{Scrubber, find_first_image_in_directory};
use crate::shortcuts::InputEvent::*;
mod utils;
use utils::*;
mod appstate;
mod image_loader;
use appstate::*;
// mod events;
#[cfg(target_os = "macos")]
mod mac;
mod net;
use net::*;
#[cfg(test)]
mod tests;
mod ui;
#[cfg(feature = "update")]
mod update;
use ui::*;

mod db;
use crate::db::{DB, get_db_file};
use crate::image_editing::EditState;

mod image_editing;
pub mod paint;

pub const FONT: &[u8; 309828] = include_bytes!("../res/fonts/Inter-Regular.ttf");
const STAR: ui::Star = ui::Star {
    spikes: 5,
    outer_radius: 20.,
    inner_radius: 10.,
    x: 50.,
    y: 60.,
    stroke: 3.,
};
const TOP_MENU_HEIGHT: f32 = 30.;


#[notan_main]
fn main() -> Result<(), String> {
    env_logger::builder()
        .format(|buf, record| {
            writeln!(buf,
                     "{}:{} {} - {}",
                     record.file().unwrap_or("unknown"),
                     record.line().unwrap_or(0),
                     record.level(),
                     record.args()
            )
        })
        .filter(Some("oculante"), log::LevelFilter::Debug)
        .init();

    if std::env::var("RUST_LOG").is_err() {
        std::env::set_var("RUST_LOG", "warning");
    }
    // on debug builds, override log level
    #[cfg(debug_assertions)]
    {
        std::env::set_var("RUST_LOG", "oculante=debug");
        let _ = env_logger::try_init();
    }

    let icon_data = include_bytes!("../icon.ico");

    let mut window_config = WindowConfig::new()
        .title(&format!("Oculante | {}", env!("CARGO_PKG_VERSION")))
        .size(1026, 600) // window's size
        .resizable(true) // window can be resized
        .set_window_icon_data(Some(icon_data))
        .set_taskbar_icon_data(Some(icon_data))
        .multisampling(0)
        .min_size(200, 200);

    #[cfg(target_os = "windows")]
    {
        window_config = window_config.vsync(true).high_dpi(true);
    }

    #[cfg(target_os = "linux")]
    {
        window_config = window_config.lazy_loop(false).vsync(true).high_dpi(true);
    }

    #[cfg(target_os = "netbsd")]
    {
        window_config = window_config.lazy_loop(false).vsync(true);
    }

    #[cfg(target_os = "macos")]
    {
        window_config = window_config.lazy_loop(false).vsync(true).high_dpi(true);
    }

    #[cfg(target_os = "macos")]
    {
        // MacOS needs an incredible dance performed just to open a file
        let _ = mac::launch();
    }

    // Unfortunately we need to load the persistent settings here, too - the window settings need
    // to be set before window creation
    match settings::PersistentSettings::load() {
        Ok(settings) => {
            window_config.vsync = settings.vsync;
            if settings.window_geometry != Default::default() {
                window_config.width = settings.window_geometry.1 .0;
                window_config.height = settings.window_geometry.1 .1;
            }
            debug!("Loaded settings.");
            if settings.zen_mode {
                let mut title_string = window_config.title.clone();
                title_string.push_str(&format!(
                    "          '{}' to disable zen mode",
                    shortcuts::lookup(&settings.shortcuts, &shortcuts::InputEvent::ZenMode)
                ));
                window_config = window_config.title(&title_string);
            }
        }
        Err(e) => {
            error!("Could not load settings: {e}");
        }
    }
    window_config.always_on_top = true;

    info!("Starting oculante.");
    notan::init_with(init)
        .add_config(window_config)
        .add_config(EguiConfig)
        .add_config(DrawConfig)
        .event(event)
        .update(update)
        .draw(drawe)
        .build()
}

fn init(gfx: &mut Graphics, plugins: &mut Plugins) -> OculanteState {
    info!("Now matching arguments {:?}", std::env::args());
    // Filter out strange mac args
    let args: Vec<String> = std::env::args().filter(|a| !a.contains("psn_")).collect();

    let matches = Command::new("Oculante")
        .arg(
            Arg::new("INPUT")
                .help("Display this image")
                // .required(true)
                .index(1),
        )
        .arg(
            Arg::new("l")
                .short('l')
                .help("Listen on port")
                .takes_value(true),
        )
        .arg(
            Arg::new("chainload")
                .required(false)
                .takes_value(false)
                .short('c')
                .help("Chainload on Mac"),
        )
        .get_matches_from(args);

    debug!("Completed argument parsing.");

    let maybe_img_location = matches.value_of("INPUT").map(PathBuf::from);

    let mut state = OculanteState {
        texture_channel: mpsc::channel(),
        // current_path: maybe_img_location.cloned(/),
        ..Default::default()
    };

    match settings::PersistentSettings::load() {
        Ok(settings) => {
            state.persistent_settings = settings;
            info!("Successfully loaded previous settings.")
        }
        Err(e) => {
            warn!("Settings failed to load: {e}. This may happen after application updates. Generating a fresh file.");
            state.persistent_settings = Default::default();
            state.persistent_settings.save();
        }
    }

    state.player = Player::new(
        state.texture_channel.0.clone(),
        state.persistent_settings.max_cache,
        gfx.limits().max_texture_size,
    );

    if let Some(ref location) = maybe_img_location {
        // Check if path is a directory or a file (and that it even exists)
        let mut start_img_location: Option<PathBuf> = None;

        if let Ok(maybe_location_metadata) = location.metadata() {
            if maybe_location_metadata.is_dir() {
                // Folder - Pick first image from the folder...
                if let Ok(first_img_location) = find_first_image_in_directory(location) {
                    start_img_location = Some(first_img_location);
                }
            } else if is_ext_compatible(location) {
                // Image File with a usable extension
                start_img_location = Some(location.clone());
            } else {
                // Unsupported extension
                state.send_message(&format!("ERROR: Unsupported file: {} - Open Github issue if you think this should not happen.", location.display()));
            }
        } else {
            // Not a valid path, or user doesn't have permission to access?
            state.send_message(&format!("ERROR: Can't open file: {}", location.display()));
        }

        // Assign image path if we have a valid one here
        if let Some(img_location) = start_img_location {
            state.is_loaded = false;
            state.current_path = Some(img_location.clone());
            state
                .player
                .load(&img_location, state.message_channel.0.clone());
        }
    }

    if let Some(port) = matches.value_of("l") {
        match port.parse::<i32>() {
            Ok(p) => {
                state.message = Some(Message::info(&format!("Listening on {p}")));
                recv(p, state.texture_channel.0.clone());
                state.current_path = Some(PathBuf::from(&format!("network port {p}")));
                state.network_mode = true;
            }
            Err(_) => error!("Port must be a number"),
        }
    }

    // Set up egui style
    plugins.egui(|ctx| {
        let mut fonts = FontDefinitions::default();

        fonts
            .font_data
            .insert("my_font".to_owned(), FontData::from_static(FONT));

        // TODO: This needs to be a monospace font
        // fonts.font_data.insert(
        //     "my_font_mono".to_owned(),
        //     FontData::from_static(include_bytes!("../res/fonts/FiraCode-Regular.ttf"))
        // );

        // Put my font first (highest priority):
        fonts
            .families
            .get_mut(&FontFamily::Proportional)
            .unwrap()
            .insert(0, "my_font".to_owned());

        // fonts.families.get_mut(&FontFamily::Monospace).unwrap()
        //     .insert(0, "my_font_mono".to_owned());

        let mut style: egui::Style = (*ctx.style()).clone();
        let font_scale = 0.80;

        style.text_styles.get_mut(&TextStyle::Body).unwrap().size = 18. * font_scale;
        style.text_styles.get_mut(&TextStyle::Button).unwrap().size = 18. * font_scale;
        style.text_styles.get_mut(&TextStyle::Small).unwrap().size = 15. * font_scale;
        style.text_styles.get_mut(&TextStyle::Heading).unwrap().size = 22. * font_scale;
        style.visuals.selection.bg_fill = Color32::from_rgb(
            state.persistent_settings.accent_color[0],
            state.persistent_settings.accent_color[1],
            state.persistent_settings.accent_color[2],
        );

        let accent_color = style.visuals.selection.bg_fill.to_array();

        let accent_color_luma = (accent_color[0] as f32 * 0.299
            + accent_color[1] as f32 * 0.587
            + accent_color[2] as f32 * 0.114)
            .max(0.)
            .min(255.) as u8;
        let accent_color_luma = if accent_color_luma < 80 { 220 } else { 80 };
        // Set text on highlighted elements
        style.visuals.selection.stroke = Stroke::new(2.0, Color32::from_gray(accent_color_luma));
        ctx.set_style(style);
        ctx.set_fonts(fonts);
    });

    // load checker texture
    if let Ok(checker_image) = image::load_from_memory(include_bytes!("../res/checker.png")) {
        // state.checker_texture = checker_image.into_rgba8().to_texture(gfx);
        // No mipmaps for the checker pattern!
        let img = checker_image.into_rgba8();
        state.checker_texture = gfx
            .create_texture()
            .from_bytes(&img, img.width() as i32, img.height() as i32)
            .with_mipmaps(false)
            .with_format(notan::prelude::TextureFormat::SRgba8)
            .build()
            .ok();
    }

    state
}

fn event(app: &mut App, state: &mut OculanteState, evt: Event) {
    match evt {
        Event::KeyUp { .. } => {
            // Fullscreen needs to be on key up on mac (bug)
            if key_pressed(app, state, Fullscreen) {
                toggle_fullscreen(app, state);
            }
        }
        Event::KeyDown { .. } => {
            // return;
            // pan image with keyboard
            if state.key_grab {
                state.send_message_err("Shortcuts don't work, key grab is active");
            }

            if state.message.is_some() {
                state.message = None;
            }

            let delta = 40.;
            if key_pressed(app, state, PanRight) {
                state.image_geometry.offset.x += delta;
                limit_offset(app, state);
            }
            if key_pressed(app, state, PanUp) {
                state.image_geometry.offset.y -= delta;
                limit_offset(app, state);
            }
            if key_pressed(app, state, PanLeft) {
                state.image_geometry.offset.x -= delta;
                limit_offset(app, state);
            }
            if key_pressed(app, state, PanDown) {
                state.image_geometry.offset.y += delta;
                limit_offset(app, state);
            }
            if key_pressed(app, state, CompareNext) {
                compare_next(state);
            }
            if key_pressed(app, state, ResetView) {
                state.reset_image = true
            }
            if key_pressed(app, state, ZenMode) {
                toggle_zen_mode(state, app);
            }
            if key_pressed(app, state, ZoomActualSize) {
                set_zoom(1.0, None, state);
            }
            if key_pressed(app, state, ZoomDouble) {
                set_zoom(2.0, None, state);
            }
            if key_pressed(app, state, ZoomThree) {
                set_zoom(3.0, None, state);
            }
            if key_pressed(app, state, ZoomFour) {
                set_zoom(4.0, None, state);
            }
            if key_pressed(app, state, ZoomFive) {
                set_zoom(5.0, None, state);
            }
            if key_pressed(app, state, Favourite) {
                add_to_favourites(state);
            }
            if key_pressed(app, state, ToggleSlideshow) {
                state.toggle_slideshow = !state.toggle_slideshow;
            }
            if key_pressed(app, state, DeleteFile) {
                if let Some(img_path) = &state.current_path {
                    trash::delete(img_path).expect("Cannot delete file");
                    state.send_message(format!("file {:?} removed", img_path).as_str());
                    state.scrubber.delete(img_path);
                    state.reload_image();
                }
            }
            if key_pressed(app, state, Quit) {
                state.persistent_settings.save_blocking();

                if let Some(ref mut db) = state.db {
                    db.close();
                }

                app.backend.exit();
            }
            #[cfg(feature = "turbo")]
            if key_pressed(app, state, LosslessRotateRight) {
                debug!("Lossless rotate right");

                if let Some(p) = &state.current_path {
                    if lossless_tx(
                        p,
                        turbojpeg::Transform {
                            op: turbojpeg::TransformOp::Rot90,
                            ..turbojpeg::Transform::default()
                        },
                    )
                    .is_ok()
                    {
                        state.is_loaded = false;
                        // This needs "deep" reload
                        state.player.cache.clear();
                        state.player.load(p, state.message_channel.0.clone());
                    }
                }
            }
            #[cfg(feature = "turbo")]
            if key_pressed(app, state, LosslessRotateLeft) {
                debug!("Lossless rotate left");
                if let Some(p) = &state.current_path {
                    if lossless_tx(
                        p,
                        turbojpeg::Transform {
                            op: turbojpeg::TransformOp::Rot270,
                            ..turbojpeg::Transform::default()
                        },
                    )
                    .is_ok()
                    {
                        state.is_loaded = false;
                        // This needs "deep" reload
                        state.player.cache.clear();
                        state.player.load(p, state.message_channel.0.clone());
                    } else {
                        warn!("rotate left failed")
                    }
                }
            }
            #[cfg(feature = "file_open")]
            if key_pressed(app, state, Browse) {
                browse_for_image_path(state, app)
            }
            #[cfg(feature = "file_open")]
            if key_pressed(app, state, BrowseFolder) {
                browse_for_folder_path(state, app)
            }
            if key_pressed(app, state, CopyImagePathToClipboard) {
                if let Some(img_path) = &state.current_path {
                    let mut ctx: ClipboardContext = ClipboardProvider::new().expect("Cannot create Clipboard context");
                    ctx.set_contents(img_path.to_string_lossy().to_string()).expect("Cannot set Clipboard context");
                    state.send_message(format!("path {:?} copied", img_path).as_str());
                }
            }
            if key_pressed(app, state, NextImage) {
                if state.is_loaded {
                    next_image(state)
                }
            }
            if key_pressed(app, state, PreviousImage) {
                if state.is_loaded {
                    prev_image(state)
                }
            }
            if key_pressed(app, state, FirstImage) {
                first_image(state)
            }
            if key_pressed(app, state, LastImage) {
                last_image(state)
            }
            if key_pressed(app, state, AlwaysOnTop) {
                state.always_on_top = !state.always_on_top;
                app.window().set_always_on_top(state.always_on_top);
            }
            if key_pressed(app, state, InfoMode) {
                state.persistent_settings.info_enabled = !state.persistent_settings.info_enabled;
                send_extended_info(
                    &state.current_image,
                    &state.current_path,
                    &state.extended_info_channel,
                );
            }
            if key_pressed(app, state, EditMode) {
                state.persistent_settings.edit_enabled = !state.persistent_settings.edit_enabled;
            }
            if key_pressed(app, state, ZoomIn) {
                let delta = zoomratio(3.5, state.image_geometry.scale);
                let new_scale = state.image_geometry.scale + delta;
                // limit scale
                if new_scale > 0.05 && new_scale < 40. {
                    // We want to zoom towards the center
                    let center: Vector2<f32> = nalgebra::Vector2::new(
                        app.window().width() as f32 / 2.,
                        app.window().height() as f32 / 2.,
                    );
                    state.image_geometry.offset -= scale_pt(
                        state.image_geometry.offset,
                        center,
                        state.image_geometry.scale,
                        delta,
                    );
                    state.image_geometry.scale += delta;
                }
            }
            if key_pressed(app, state, ZoomOut) {
                let delta = zoomratio(-3.5, state.image_geometry.scale);
                let new_scale = state.image_geometry.scale + delta;
                // limit scale
                if new_scale > 0.05 && new_scale < 40. {
                    // We want to zoom towards the center
                    let center: Vector2<f32> = nalgebra::Vector2::new(
                        app.window().width() as f32 / 2.,
                        app.window().height() as f32 / 2.,
                    );
                    state.image_geometry.offset -= scale_pt(
                        state.image_geometry.offset,
                        center,
                        state.image_geometry.scale,
                        delta,
                    );
                    state.image_geometry.scale += delta;
                }
            }
        }
        Event::WindowResize { width, height } => {
            //TODO: remove this if save on exit works
            state.persistent_settings.window_geometry.1 = (width, height);
            state.persistent_settings.window_geometry.0 = app.backend.window().position();
            // By resetting the image, we make it fill the window on resize
            if state.persistent_settings.zen_mode {
                state.reset_image = true;
            }
        }
        _ => (),
    }

    match evt {
        Event::Exit => {
            debug!("About to exit");
            // save position
            state.persistent_settings.window_geometry =
                (app.window().position(), app.window().size());
            state.persistent_settings.save_blocking();

            if let Some(ref mut db) = state.db {
                db.close();
            }
        }
        Event::MouseWheel { delta_y, .. } => {
            if !state.pointer_over_ui {
                if app.keyboard.ctrl() {
                    // Change image to next/prev
                    // - map scroll-down == next, as that's the natural scrolling direction
                    if delta_y > 0.0 {
                        prev_image(state)
                    } else {
                        next_image(state)
                    }
                } else {
                    // Normal scaling
                    let delta = zoomratio(
                        (delta_y / 10.).max(-5.0).min(5.0),
                        state.image_geometry.scale,
                    );
                    let new_scale = state.image_geometry.scale + delta;
                    // limit scale
                    if new_scale > 0.01 && new_scale < 40. {
                        state.image_geometry.offset -= scale_pt(
                            state.image_geometry.offset,
                            state.cursor,
                            state.image_geometry.scale,
                            delta,
                        );
                        state.image_geometry.scale += delta;
                    }
                }
            }
        }

        Event::Drop(file) => {
            if let Some(p) = file.path {
                if let Some(ext) = p.extension() {
                    if SUPPORTED_EXTENSIONS.contains(&ext.to_string_lossy().to_string().as_str()) {
                        state.is_loaded = false;
                        state.current_image = None;
                        state.player.load(&p, state.message_channel.0.clone());
                        state.current_path = Some(p);
                    } else {
                        state.message = Some(Message::warn("Unsupported file!"));
                    }
                }
            }
        }
        Event::MouseDown { button, .. } => {
            if state.cursor_within_image() {
                match button {
                    MouseButton::Left => {
                        if !state.mouse_grab {
                            state.drag_enabled = true;
                        }
                    }
                    MouseButton::Middle => {
                        state.show_metadata_tooltip = !state.show_metadata_tooltip;
                    }
                    MouseButton::Right => {
                        if state.current_image.is_some() {
                            if round(state.image_geometry.offset[1] as f64, 4) == 0. {
                                set_zoom(1., None, state);
                            } else {
                                state.reset_image = true;
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        Event::MouseUp { button, .. } => match button {
            MouseButton::Left | MouseButton::Middle => state.drag_enabled = false,
            _ => {}
        },
        _ => {
            // debug!("{:?}", evt);
        }
    }
}

fn update(app: &mut App, state: &mut OculanteState) {
    if state.first_start {
        app.window().set_always_on_top(false);
    }

    let mouse_pos = app.mouse.position();

    state.mouse_delta = Vector2::new(mouse_pos.0, mouse_pos.1) - state.cursor;
    state.cursor = mouse_pos.size_vec();
    if state.drag_enabled {
        if !state.mouse_grab || app.mouse.is_down(MouseButton::Middle) {
            state.image_geometry.offset += state.mouse_delta;
            limit_offset(app, state);
        }
    }

    // Since we can't access the window in the event loop, we store it in the state
    state.window_size = app.window().size().size_vec();

    if state.persistent_settings.info_enabled || state.edit_state.painting {
        state.cursor_relative = pos_from_coord(
            state.image_geometry.offset,
            state.cursor,
            Vector2::new(
                state.image_dimension.0 as f32,
                state.image_dimension.1 as f32,
            ),
            state.image_geometry.scale,
        );
    }

    // make sure that in edit mode, RGBA is set.
    // This is a bit lazy. but instead of writing lots of stuff for an ubscure feature,
    // let's disable it here.
    if state.persistent_settings.edit_enabled {
        state.persistent_settings.current_channel = ColorChannel::Rgba;
    }

    // redraw if extended info is missing so we make sure it's promply displayed
    if state.persistent_settings.info_enabled && state.image_info.is_none() {
        app.window().request_frame();
    }

    // check extended info has been sent
    if let Ok(info) = state.extended_info_channel.1.try_recv() {
        debug!("Received extended image info for {}", info.name);
        state.image_info = Some(info);
        app.window().request_frame();
    }

    // Only receive messages if current one is cleared
    // debug!("cooldown {}", state.toast_cooldown);

    // check if a new message has been sent
    if let Ok(msg) = state.message_channel.1.try_recv() {
        debug!("Received message: {:?}", msg);
        state.toast_cooldown = Instant::now();

        if let Message::LoadError(_) = msg {
            state.current_image = None;
            state.is_loaded = true;
            state.current_texture = None;
            set_title(app, state);
        }

        state.message = Some(msg);
    }

    state.first_start = false;

    if state.toggle_slideshow
        && state.is_loaded
        && state.slideshow_time.elapsed() >= Duration::from_secs(state.persistent_settings.slideshow_delay)
    {
        next_image(state);
        state.slideshow_time = Instant::now();
    }
}

fn drawe(app: &mut App, gfx: &mut Graphics, plugins: &mut Plugins, state: &mut OculanteState) {
    let mut draw = gfx.create_draw();

    if let Ok(p) = state.load_channel.1.try_recv() {
        state.is_loaded = false;
        state.current_image = None;
        state.player.load(&p, state.message_channel.0.clone());
        if let Some(dir) = p.parent() {
            state.persistent_settings.last_open_directory = dir.to_path_buf();
        }
        state.current_path = Some(p);
        _ = state.persistent_settings.save();
    }

    // check if a new texture has been sent
    if let Ok(frame) = state.texture_channel.1.try_recv() {
        let img = frame.buffer;
        // debug!("Received image buffer: {:?}", img.dimensions());
        state.image_dimension = img.dimensions();
        // state.current_texture = img.to_texture(gfx);

        // debug!("Frame source: {:?}", frame.source);

        set_title(app, state);

        if let Some(current_path) = &state.current_path {
            if state.scrubber.favourites.contains(current_path) {
                state.current_image_is_favourite = true;
            } else {
                state.current_image_is_favourite = false;
            }

            match fs::metadata(current_path) {
                Ok(metadata) => {
                    state.image_size = metadata.len();
                }
                Err(_) => state.send_message_err("Couldn't get metadata"),
            }

        }

        // fill image sequence
        if state.folder_selected.is_none() {
            if let Some(p) = &state.current_path {
                state.scrubber = scrubber::Scrubber::new(p, false, false, None, 0);
                state.scrubber.wrap = state.persistent_settings.wrap_folder;

                // debug!("{:#?} from {}", &state.scrubber, p.display());
                if !state.persistent_settings.recent_images.contains(p) {
                    state.persistent_settings.recent_images.insert(0, p.clone());
                    state.persistent_settings.recent_images.truncate(10);
                }
            }
        }

        match frame.source {
            FrameSource::Still => {
                state.edit_state.result_image_op = Default::default();
                state.edit_state.result_pixel_op = Default::default();

                if !state.persistent_settings.keep_view {
                    state.reset_image = true;

                    if let Some(p) = state.current_path.clone() {
                        if state.persistent_settings.max_cache != 0 {
                            state.player.cache.insert(&p, img.clone());
                        }
                    }
                }
                // always reset if first image
                if state.current_texture.is_none() {
                    state.reset_image = true;
                }

                if !state.persistent_settings.keep_edits {
                    state.edit_state = Default::default();
                } else {
                    state.edit_state.result_pixel_op = Default::default();
                    state.edit_state.result_image_op = Default::default();
                }

                // Load edit information if any
                if let Some(p) = &state.current_path {
                    if p.with_extension("oculante").is_file() {
                        if let Ok(f) = std::fs::File::open(p.with_extension("oculante")) {
                            if let Ok(edit_state) = serde_json::from_reader::<_, EditState>(f) {
                                state.send_message("Edits have been loaded for this image.");
                                state.edit_state = edit_state;
                                state.persistent_settings.edit_enabled = true;
                                state.reset_image = true;
                            }
                        }
                    } else if let Some(parent) = p.parent() {
                        if parent.join(".oculante").is_file() {
                            info!("is file {}", parent.join(".oculante").display());

                            if let Ok(f) = std::fs::File::open(parent.join(".oculante")) {
                                if let Ok(edit_state) = serde_json::from_reader::<_, EditState>(f) {
                                    state.send_message(
                                        "Directory edits have been loaded for this image.",
                                    );
                                    state.edit_state = edit_state;
                                    state.persistent_settings.edit_enabled = true;
                                    state.reset_image = true;
                                }
                            }
                        }
                    }
                }
                state.redraw = false;
                state.image_info = None;
            }
            FrameSource::EditResult => {
                // debug!("EditResult");
                // state.edit_state.is_processing = false;
            }
            FrameSource::AnimationStart => {
                state.redraw = true;
                state.reset_image = true
            }
            FrameSource::Animation => {
                state.redraw = true;
            }
        }

        if let Some(tex) = &mut state.current_texture {
            if tex.width() as u32 == img.width() && tex.height() as u32 == img.height() {
                img.update_texture(gfx, tex);
            } else {
                state.current_texture = img.to_texture(gfx);
            }
        } else {
            state.current_texture = img.to_texture(gfx);
        }

        state.is_loaded = true;

        match &state.persistent_settings.current_channel {
            // Unpremultiply the image
            ColorChannel::Rgb => state.current_texture = unpremult(&img).to_texture(gfx),
            // Do nuttin'
            ColorChannel::Rgba => (),
            // Display the channel
            _ => {
                state.current_texture =
                    solo_channel(&img, state.persistent_settings.current_channel as usize)
                        .to_texture(gfx)
            }
        }
        state.current_image = Some(img);
        if state.persistent_settings.info_enabled {
            debug!("Sending extended info");
            send_extended_info(
                &state.current_image,
                &state.current_path,
                &state.extended_info_channel,
            );
        }
    }

    if state.redraw {
        debug!("Force redraw");
        app.window().request_frame();
    }

    if state.reset_image {
        let window_size = app.window().size().size_vec();
        if let Some(current_image) = &state.current_image {
            let img_size = current_image.size_vec();
            let scale_factor = (window_size.x / img_size.x)
                .min(window_size.y / img_size.y)
                .min(1.0);
            state.image_geometry.scale = scale_factor;
            state.image_geometry.offset =
                window_size / 2.0 - (img_size * state.image_geometry.scale) / 2.0;

            state.reset_image = false;
        }
        // app.window().request_frame();
    }

    // TODO: Do we need/want a "global" checker?
    // if state.persistent_settings.show_checker_background {
    //     if let Some(checker) = &state.checker_texture {
    //         draw.pattern(checker)
    //             .blend_mode(BlendMode::ADD)
    //             .size(app.window().width() as f32, app.window().height() as f32);
    //     }
    // }

    if let Some(texture) = &state.current_texture {
        if state.persistent_settings.show_checker_background {
            if let Some(checker) = &state.checker_texture {
                draw.pattern(checker)
                    // .size(texture.width() as f32, texture.height() as f32)
                    .size(texture.width() as f32 * state.image_geometry.scale * state.tiling as f32, texture.height() as f32 * state.image_geometry.scale* state.tiling as f32)
                    .blend_mode(BlendMode::ADD)
                    .translate(state.image_geometry.offset.x, state.image_geometry.offset.y)
                    // .scale(state.image_geometry.scale, state.image_geometry.scale)
                    ;
            }
        }
        if state.tiling < 2 {
            draw.image(texture)
                .blend_mode(BlendMode::NORMAL)
                .translate(state.image_geometry.offset.x, state.image_geometry.offset.y)
                .scale(state.image_geometry.scale, state.image_geometry.scale);
        } else {
            draw.pattern(texture)
                .translate(state.image_geometry.offset.x, state.image_geometry.offset.y)
                .scale(state.image_geometry.scale, state.image_geometry.scale)
                .size(
                    texture.width() * state.tiling as f32,
                    texture.height() * state.tiling as f32,
                );
        }

        if state.persistent_settings.show_frame {
            draw.rect((0.0, 0.0), texture.size())
                .stroke(1.0)
                .color(Color {
                    r: 0.5,
                    g: 0.5,
                    b: 0.5,
                    a: 0.5,
                })
                .blend_mode(BlendMode::ADD)
                .translate(state.image_geometry.offset.x, state.image_geometry.offset.y)
                .scale(state.image_geometry.scale, state.image_geometry.scale);
        }

        if state.persistent_settings.show_minimap {
            // let offset_x = app.window().size().0 as f32 - state.image_dimension.0 as f32;
            let offset_x = 0.0;

            let scale = 200. / app.window().size().0 as f32;
            let show_minimap = state.image_dimension.0 as f32 * state.image_geometry.scale
                > app.window().size().0 as f32;

            if show_minimap {
                draw.image(texture)
                    .blend_mode(BlendMode::NORMAL)
                    .translate(offset_x, 100.)
                    .scale(scale, scale);
            }
        }

        // Draw a brush preview when paint mode is on
        if state.edit_state.painting {
            if let Some(stroke) = state.edit_state.paint_strokes.last() {
                let dim = texture.width().min(texture.height()) / 50.;
                draw.circle(20.)
                    // .translate(state.cursor_relative.x, state.cursor_relative.y)
                    .translate(state.cursor.x, state.cursor.y)
                    .alpha(0.5)
                    .stroke(1.5)
                    .scale(state.image_geometry.scale, state.image_geometry.scale)
                    .scale(stroke.width * dim, stroke.width * dim);

                // For later: Maybe paint the actual brush? Maybe overkill.

                // if let Some(brush) = state.edit_state.brushes.get(stroke.brush_index) {
                //     if let Some(brush_tex) = brush.to_texture(gfx) {
                //         draw.image(&brush_tex)
                //             .blend_mode(BlendMode::NORMAL)
                //             .translate(state.cursor.x, state.cursor.y)
                //             .scale(state.scale, state.scale)
                //             .scale(stroke.width*dim, stroke.width*dim)
                //             // .translate(state.offset.x as f32, state.offset.y as f32)
                //             // .transform(state.cursor_relative)
                //             ;
                //     }
                // }
            }
        }

        if state.current_image_is_favourite {
            draw.star(STAR.spikes, STAR.outer_radius, STAR.inner_radius)
                .position(STAR.x, STAR.y)
                .fill_color(Color::YELLOW)
                .fill()
                .stroke_color(Color::ORANGE)
                .stroke(STAR.stroke);
        }
    }

    let egui_output = plugins.egui(|ctx| {
        // the top menu bar

        if state.show_metadata_tooltip && !state.pointer_over_ui && state.cursor_within_image() {
            let pos_y = TOP_MENU_HEIGHT + 5. + if state.current_image_is_favourite { STAR.y } else { 0. };

            show_tooltip_at(
                ctx,
                egui::Id::new("my_tooltip"),
                Some(pos2(10., pos_y)),
                |ui| {
                    ui.label(format!(
                        "scale: {scale}\nwidth: {width}\nheight: {height}\nsize: {size}",
                        scale = round(state.image_geometry.scale as f64, 2),
                        width = state.image_dimension.0,
                        height = state.image_dimension.1,
                        size = format_bytes(state.image_size as f64),
                    ));
                },
            );
        }

        if !state.persistent_settings.zen_mode {
            egui::TopBottomPanel::top("menu")
                .min_height(TOP_MENU_HEIGHT)
                .default_height(TOP_MENU_HEIGHT)
                .show(ctx, |ui| {
                    main_menu(ui, state, app, gfx);
                });
        }

        if state.persistent_settings.show_scrub_bar {
            egui::TopBottomPanel::bottom("scrubber")
                .max_height(22.)
                .min_height(22.)
                .show(ctx, |ui| {
                    scrubber_ui(state, ui);
                });
        }

        if let Some(message) = &state.message.clone() {
            // debug!("Message is set, showing");
            egui::TopBottomPanel::bottom("message").show_animated(
                ctx,
                state.message.is_some(),
                |ui| {
                    ui.horizontal(|ui| {
                        match message {
                            Message::Info(txt) => {
                                ui.label(format!("💬 {txt}"));
                            }
                            Message::Warning(txt) => {
                                ui.colored_label(Color32::GOLD, format!("⚠ {txt}"));
                            }
                            Message::Error(txt) | Message::LoadError(txt) => {
                                ui.colored_label(Color32::RED, format!("🕱 {txt}"));
                            }
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                            if ui.small_button("🗙").clicked() {
                                state.message = None
                            }
                        });
                    });

                    ui.ctx().request_repaint();
                },
            );

            // using delta does not work with rfd
            // state.toast_cooldown += app.timer.delta_f32();
            // debug!("cooldown {}", state.toast_cooldown);

            if state.toast_cooldown.elapsed() > Duration::from_secs(3u64) {
                state.message = None;
            }
        }

        if state.persistent_settings.info_enabled
            && !state.settings_enabled
            && !state.persistent_settings.zen_mode
        {
            info_ui(ctx, state, gfx);
        }

        if state.persistent_settings.edit_enabled
            && !state.settings_enabled
            && !state.persistent_settings.zen_mode
        {
            edit_ui(app, ctx, state, gfx);
        }

        if !state.is_loaded && !state.toggle_slideshow {
            egui::TopBottomPanel::bottom("loader").show_animated(
                ctx,
                state.current_path.is_some(),
                |ui| {
                    if let Some(p) = &state.current_path {
                        ui.horizontal(|ui| {
                            ui.add(egui::Spinner::default());
                            ui.label(format!("Loading {}", p.display()));
                        });
                    }
                    app.window().request_frame();
                },
            );
        }

        state.pointer_over_ui = ctx.is_pointer_over_area();
        // info!("using pointer {}", ctx.is_using_pointer());

        // if there is interaction on the ui (dragging etc)
        // we don't want zoom & pan to work, so we "grab" the pointer
        if ctx.is_using_pointer() || state.edit_state.painting || ctx.is_pointer_over_area() {
            state.mouse_grab = true;
        } else {
            state.mouse_grab = false;
        }

        if ctx.wants_keyboard_input() {
            state.key_grab = true;
        } else {
            state.key_grab = false;
        }
        // Settings come last, as they block keyboard grab (for hotkey assigment)
        settings_ui(app, ctx, state);
    });

    if state.network_mode {
        app.window().request_frame();
    }
    // if state.edit_state.is_processing {
    //     app.window().request_frame();
    // }
    let c = state.persistent_settings.background_color;
    // draw.clear(Color:: from_bytes(c[0], c[1], c[2], 255));
    draw.clear(Color::from_rgb(
        c[0] as f32 / 255.,
        c[1] as f32 / 255.,
        c[1] as f32 / 255.,
    ));
    gfx.render(&draw);
    gfx.render(&egui_output);
    if egui_output.needs_repaint() {
        app.window().request_frame();
    }
}

// Show file browser to select image to load
#[cfg(feature = "file_open")]
fn browse_for_image_path(state: &mut OculanteState, app: &mut App) {
    app.keyboard.down = Default::default();
    let start_directory = &state.persistent_settings.last_open_directory;

    let file_dialog_result = rfd::FileDialog::new()
        .add_filter("All Supported Image Types", utils::SUPPORTED_EXTENSIONS)
        .add_filter("All File Types", &["*"])
        .set_directory(start_directory)
        .pick_file();

    if let Some(file_path) = file_dialog_result {
        let mut current_path = file_path.clone();
        state.folder_selected = None;

        if file_path.extension() == Some(OsStr::new("txt")) {
            let file = File::open(&file_path).unwrap();
            let reader = io::BufReader::new(file);
            let img_paths: Vec<PathBuf> = reader
                .lines()
                .into_iter()
                .filter_map(|line| {line.ok()})
                .map(|line| {Path::new(&line).to_path_buf()})
                .collect();

            state.folder_selected = Some(file_path.parent().unwrap().to_path_buf());
            state.scrubber = Scrubber::new_from_entries(img_paths);
            current_path = state.scrubber.get(0).unwrap();
        }

        state.is_loaded = false;
        state.current_image = None;
        state
            .player
            .load(&current_path, state.message_channel.0.clone());
        if let Some(dir) = file_path.parent() {
            state.persistent_settings.last_open_directory = dir.to_path_buf();
        }
        state.current_path = Some(current_path);
        _ = state.persistent_settings.save();
    }
}

#[cfg(feature = "file_open")]
fn browse_for_folder_path(state: &mut OculanteState, app: &mut App) {
    app.keyboard.down = Default::default();
    let start_directory = &state.persistent_settings.last_open_directory;

    let folder_dialog_result = rfd::FileDialog::new()
        .set_directory(start_directory)
        .pick_folder();

    if let Some(folder_path) = folder_dialog_result {
        state.persistent_settings.last_open_directory = folder_path.clone();
        _ = state.persistent_settings.save();
        state.folder_selected = Option::from(folder_path.clone());

        let db_file = get_db_file(&folder_path);

        let favourites: Option<HashSet<PathBuf>>;
        if db_file.exists() {
            state.db = Option::from(DB::new(&folder_path));
            favourites = Option::from(state.db.as_ref().unwrap().get_all());
        } else {
            favourites = None;
        }

        state.scrubber = Scrubber::new(
            &folder_path.as_path(),
            true,
            true,
            favourites,
            state.persistent_settings.add_fav_every_n,
        );
        let number_of_files = state.scrubber.len();
        let number_of_favs = state.scrubber.favourites.len();
        if number_of_files > 0 {
            state.send_message(
                format!(
                    "files: {}, favourites: {}",
                    number_of_files,
                    number_of_favs,
                ).as_str(),
            );
            let current_path = state.scrubber.get(0).unwrap();

            state.is_loaded = false;
            state.current_image = None;
            state
                .player
                .load(current_path.as_path(), state.message_channel.0.clone());

            state.current_path = Some(current_path);
        } else {
            state.send_message_err(format!("No supported image files in {:?}", folder_path).as_str());
        }
    }
}

// Make sure offset is restricted to window size so we don't offset to infinity
fn limit_offset(app: &mut App, state: &mut OculanteState) {
    let window_size = app.window().size();
    let scaled_image_size = (
        state.image_dimension.0 as f32 * state.image_geometry.scale,
        state.image_dimension.1 as f32 * state.image_geometry.scale,
    );
    state.image_geometry.offset.x = state
        .image_geometry
        .offset
        .x
        .min(window_size.0 as f32)
        .max(-scaled_image_size.0);
    state.image_geometry.offset.y = state
        .image_geometry
        .offset
        .y
        .min(window_size.1 as f32)
        .max(-scaled_image_size.1);
}

fn set_zoom(scale: f32, from_center: Option<Vector2<f32>>, state: &mut OculanteState) {
    let delta = scale - state.image_geometry.scale;
    let zoom_point = from_center.unwrap_or(state.cursor);
    state.image_geometry.offset -= scale_pt(
        state.image_geometry.offset,
        zoom_point,
        state.image_geometry.scale,
        delta,
    );
    state.image_geometry.scale = scale;
}

fn add_to_favourites(state: &mut OculanteState) {
    if let Some(img_path) = &state.current_path {
        if state.db.is_none() {
            state.db = Option::from(DB::new(state.folder_selected.as_ref().unwrap()));
        }

        if !state.scrubber.favourites.contains(img_path) {
            state.db.as_ref().unwrap().insert(&img_path);
            state.scrubber.favourites.insert(img_path.clone());
            state.current_image_is_favourite = true;

        } else {
            state.db.as_ref().unwrap().delete(&img_path);
            state.scrubber.favourites.remove(img_path);
            state.current_image_is_favourite = false;
        }
    }
}
