use crate::audio::manager::{AudioManager, PlaybackState};
use crate::visualization::{
    renderer::WgpuSphereRenderer, sphere_geometry::generate_sphere_points_fibonacci,
};
use eframe::{egui, egui_wgpu::CallbackTrait, App, Frame};
use std::path::Path;
use std::sync::Arc;
use type_map::concurrent::TypeMap;
use wgpu;

const NUM_SPHERE_POINTS: usize = 2000;
const SPHERE_RADIUS: f32 = 1.0;

struct Custom3DPaintCallback {
    primitive: Arc<crate::visualization::renderer::SphereWgpuPrimitive>,
    mvp_matrix: glam::Mat4,
    queue: Arc<wgpu::Queue>,
}

impl CallbackTrait for Custom3DPaintCallback {
    fn paint<'a>(
        &'a self,
        _info: egui::PaintCallbackInfo,
        render_pass: &mut wgpu::RenderPass<'a>,
        // Required by the CallbackTrait paint method signature, but we use self.queue as a workaround for now.
        _resources: &'a TypeMap,
    ) {
        crate::visualization::renderer::WgpuSphereRenderer::paint_primitive(
            &self.primitive,
            &self.mvp_matrix,
            render_pass,
            // TODO: This currently works, but look at using resources.get instead.
            // Use the queue stored in this struct instead of the resources.get
            &self.queue,
        );
    }
}

// TODO: Remove `wgpu_device` if we're only going to pass it around as part of common patterns.
#[allow(dead_code)]
pub struct AudioVisualizerApp {
    file_path_input: String,
    audio_manager: Result<AudioManager, String>,
    action_error_message: Option<String>,
    sphere_renderer: WgpuSphereRenderer,
    // Store WGPU device and queue.
    wgpu_device: Option<Arc<wgpu::Device>>,
    wgpu_queue: Option<Arc<wgpu::Queue>>,
}

impl AudioVisualizerApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let sphere_points = generate_sphere_points_fibonacci(SPHERE_RADIUS, NUM_SPHERE_POINTS);
        let mut sphere_renderer = WgpuSphereRenderer::new(sphere_points);

        let mut app_wgpu_device_arc = None;
        let mut app_wgpu_queue_arc = None;

        if let Some(wgpu_render_state) = &cc.wgpu_render_state {
            // Clone Device and Queue Arcs for storing
            let device_arc = wgpu_render_state.device.clone();
            let queue_arc = wgpu_render_state.queue.clone();

            if let Err(e) =
                sphere_renderer.prepare(&device_arc, &queue_arc, wgpu_render_state.target_format)
            {
                tracing::error!("Failed to prepare WGPU sphere renderer: {}", e);
            }
            app_wgpu_device_arc = Some(device_arc);
            app_wgpu_queue_arc = Some(queue_arc);
        } else {
            tracing::warn!(
                "WGPU render state not available at creation. Visualization might not work."
            );
        }

        Self {
            file_path_input: String::new(),
            audio_manager: AudioManager::new(),
            action_error_message: None,
            sphere_renderer,
            wgpu_device: app_wgpu_device_arc,
            wgpu_queue: app_wgpu_queue_arc,
        }
    }
}

impl App for AudioVisualizerApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut Frame) {
        if let Ok(manager) = &mut self.audio_manager {
            manager.check_and_update_finished_state();
        }
        self.sphere_renderer.time += ctx.input(|i| i.stable_dt);

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Audio Visualizer");
            ui.separator();

            // Audio Controls UI
            ui.horizontal(|ui| {
                ui.label("Audio File Path (MP3):");
                ui.add_sized(
                    ui.available_size_before_wrap(),
                    egui::TextEdit::singleline(&mut self.file_path_input)
                        .hint_text("/path/to/your/audio.mp3"),
                );
            });
            ui.add_space(5.0);
            let (play_button_text, play_button_enabled) = match &self.audio_manager {
                Ok(manager) => {
                    let current_path_is_target = manager
                        .get_current_file_path()
                        .map_or(false, |p| p == &self.file_path_input);

                    match manager.get_state() {
                        PlaybackState::Idle => ("Play", !self.file_path_input.is_empty()),
                        PlaybackState::Loaded => {
                            if current_path_is_target
                                || (self.file_path_input.is_empty()
                                    && manager.get_current_file_path().is_some())
                            {
                                ("Play Loaded", true)
                            } else {
                                ("Play New File", !self.file_path_input.is_empty())
                            }
                        }
                        PlaybackState::Playing => {
                            if current_path_is_target {
                                ("Pause Current", true)
                            } else {
                                ("Play New File", !self.file_path_input.is_empty())
                            }
                        }
                        PlaybackState::Paused => {
                            if current_path_is_target
                                || (self.file_path_input.is_empty()
                                    && manager.get_current_file_path().is_some())
                            {
                                ("Resume", true)
                            } else {
                                ("Play New File", !self.file_path_input.is_empty())
                            }
                        }
                    }
                }
                Err(_) => ("Play", false),
            };
            let pause_button_explicit_enabled = match &self.audio_manager {
                Ok(manager) => manager.get_state() == PlaybackState::Playing,
                Err(_) => false,
            };
            ui.horizontal(|ui| {
                // Play Click Handler
                if ui
                    .add_enabled(play_button_enabled, egui::Button::new(play_button_text))
                    .clicked()
                {
                    if let Ok(manager) = &mut self.audio_manager {
                        self.action_error_message = None;
                        let mut op_result: Result<(), String> = Ok(());
                        let current_manager_state = manager.get_state();
                        let manager_knows_current_file = manager
                            .get_current_file_path()
                            .map_or(false, |p| p == &self.file_path_input);
                        let input_is_empty_but_manager_has_file = self.file_path_input.is_empty()
                            && manager.get_current_file_path().is_some();
                        match current_manager_state {
                            PlaybackState::Idle => {
                                op_result = manager.load_and_play_file(&self.file_path_input);
                            }
                            PlaybackState::Loaded => {
                                if manager_knows_current_file || input_is_empty_but_manager_has_file
                                {
                                    manager.resume_playback();
                                } else {
                                    op_result = manager.load_and_play_file(&self.file_path_input);
                                }
                            }
                            PlaybackState::Playing => {
                                if manager_knows_current_file {
                                    manager.pause_playback();
                                } else {
                                    op_result = manager.load_and_play_file(&self.file_path_input);
                                }
                            }
                            PlaybackState::Paused => {
                                if manager_knows_current_file || input_is_empty_but_manager_has_file
                                {
                                    manager.resume_playback();
                                } else {
                                    op_result = manager.load_and_play_file(&self.file_path_input);
                                }
                            }
                        }
                        if let Err(e) = op_result {
                            self.action_error_message = Some(e);
                        }
                    }
                }

                // Pause Click Handler
                if ui
                    .add_enabled(pause_button_explicit_enabled, egui::Button::new("Pause"))
                    .clicked()
                {
                    if let Ok(manager) = &mut self.audio_manager {
                        self.action_error_message = None;
                        manager.pause_playback();
                    }
                }
            });
            let status_message = if let Some(err_msg) = &self.action_error_message {
                format!("Error: {}", err_msg)
            } else {
                match &self.audio_manager {
                    Ok(manager) => {
                        let state = manager.get_state();
                        let file_display = manager.get_current_file_path().map_or_else(
                            || "None".to_string(),
                            |p_str| {
                                Path::new(p_str).file_name().map_or_else(
                                    || p_str.clone(),
                                    |os_str| os_str.to_string_lossy().into_owned(),
                                )
                            },
                        );
                        format!("State: {:?}, File: {}", state, file_display)
                    }
                    Err(e) => format!("Audio System Error: {}", e),
                }
            };
            ui.label(status_message);
            ui.separator();
            // End Audio Controls UI

            // Audio Visualization UI
            ui.label("3D Point Sphere Visualization:");
            let desired_size = ui.available_size_before_wrap() * egui::vec2(1.0, 0.8);
            let (rect, _response) = ui.allocate_exact_size(desired_size, egui::Sense::hover());

            if let Some(primitive_arc) = self.sphere_renderer.get_primitive_arc() {
                if let Some(queue_arc_from_app) = &self.wgpu_queue {
                    let aspect_ratio = rect.width() / rect.height();
                    let mvp_matrix = self.sphere_renderer.calculate_mvp(aspect_ratio);

                    let cb = eframe::egui_wgpu::Callback::new_paint_callback(
                        rect,
                        Custom3DPaintCallback {
                            primitive: primitive_arc,
                            mvp_matrix,
                            queue: queue_arc_from_app.clone(),
                        },
                    );
                    ui.painter().add(cb);
                } else {
                    // If we get here, WGPU initialization failed in `AudioVisualizerApp::new`
                    ui.painter().rect_filled(rect, 0.0, egui::Color32::DARK_RED);
                    ui.painter().text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        "WGPU Queue not available for rendering",
                        egui::FontId::default(),
                        egui::Color32::WHITE,
                    );
                }
            } else {
                ui.painter()
                    .rect_filled(rect, 0.0, egui::Color32::DARK_GRAY);
                ui.painter().text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "Renderer not initialized",
                    egui::FontId::default(),
                    egui::Color32::WHITE,
                );
            }
        });

        ctx.request_repaint_after(std::time::Duration::from_millis(16));
    }
}
