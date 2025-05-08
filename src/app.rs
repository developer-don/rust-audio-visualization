use crate::audio::{AudioAnalysisData, AudioManager, PlaybackState};
use crate::visualization::{
    renderer::WgpuSphereRenderer, sphere_geometry::generate_sphere_points_fibonacci,
};
use eframe::{egui, egui_wgpu::CallbackTrait, App, Frame};
use parking_lot::Mutex;
use std::path::Path;
use std::sync::{mpsc, Arc};
use type_map::concurrent::TypeMap;
use wgpu;

const NUM_SPHERE_POINTS: usize = 2000;
const SPHERE_RADIUS: f32 = 1.0;
const DEFAULT_VOLUME: Option<f32> = Some(0.25);

// Callback only needs color now, point size removed
struct Custom3DPaintCallback {
    primitive: Arc<crate::visualization::renderer::SphereWgpuPrimitive>,
    mvp_matrix: glam::Mat4,
    queue: Arc<wgpu::Queue>,
    color: [f32; 3], // Pass calculated color
                     // point_size removed
}

impl CallbackTrait for Custom3DPaintCallback {
    fn paint<'a>(
        &'a self,
        _info: egui::PaintCallbackInfo,
        render_pass: &mut wgpu::RenderPass<'a>,
        _resources: &'a TypeMap,
    ) {
        // Call the updated paint_primitive (no point_size arg)
        crate::visualization::renderer::WgpuSphereRenderer::paint_primitive(
            &self.primitive,
            &self.mvp_matrix,
            &self.color,
            // self.point_size removed
            render_pass,
            &self.queue,
        );
    }
}

pub struct AudioVisualizerApp {
    // ... fields remain the same ...
    file_path_input: String,
    audio_manager: Result<AudioManager, String>,
    action_error_message: Option<String>,
    sphere_renderer: Arc<Mutex<WgpuSphereRenderer>>,
    #[allow(dead_code)]
    wgpu_device: Option<Arc<wgpu::Device>>,
    wgpu_queue: Option<Arc<wgpu::Queue>>,
    audio_analysis_receiver: mpsc::Receiver<AudioAnalysisData>,
    analysis_sender: mpsc::SyncSender<AudioAnalysisData>,
    current_audio_data: Option<AudioAnalysisData>,
    volume: f32,
    pre_mute_volume: f32,
    is_muted: bool,
}

impl AudioVisualizerApp {
    // ... new remains the same ...
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        /* ... same as previous ... */
        let sphere_points = generate_sphere_points_fibonacci(SPHERE_RADIUS, NUM_SPHERE_POINTS);
        let mut local_sphere_renderer = WgpuSphereRenderer::new(sphere_points);
        let mut app_wgpu_device_arc = None;
        let mut app_wgpu_queue_arc = None;
        if let Some(wgpu_render_state) = &cc.wgpu_render_state {
            let device_arc = wgpu_render_state.device.clone();
            let queue_arc = wgpu_render_state.queue.clone();
            let target_format = wgpu_render_state.target_format;
            if let Err(e) = local_sphere_renderer.prepare(&device_arc, target_format) {
                tracing::error!("Failed to prepare WGPU sphere renderer: {}", e);
            } else {
                app_wgpu_device_arc = Some(device_arc);
                app_wgpu_queue_arc = Some(queue_arc);
            }
        } else {
            tracing::warn!("WGPU render state not available at creation.");
        }
        let sphere_renderer_shared = Arc::new(Mutex::new(local_sphere_renderer));
        let (analysis_sender, audio_analysis_receiver) = mpsc::sync_channel(10);

        Self {
            file_path_input: "/Users/donald/Downloads/example.mp3".to_string(),
            audio_manager: AudioManager::new(DEFAULT_VOLUME),
            action_error_message: None,
            sphere_renderer: sphere_renderer_shared,
            wgpu_device: app_wgpu_device_arc,
            wgpu_queue: app_wgpu_queue_arc,
            audio_analysis_receiver,
            analysis_sender,
            current_audio_data: None,
            volume: DEFAULT_VOLUME.unwrap_or(0.25),
            pre_mute_volume: DEFAULT_VOLUME.unwrap_or(0.25),
            is_muted: false,
        }
    }
}

impl App for AudioVisualizerApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut Frame) {
        let playback_state = self
            .audio_manager
            .as_ref()
            .map_or(PlaybackState::Idle, |m| m.get_state());
        if let Ok(manager) = &mut self.audio_manager {
            manager.check_and_update_finished_state();
        }
        while let Ok(data) = self.audio_analysis_receiver.try_recv() {
            self.current_audio_data = Some(data);
        }

        // Update Renderer State - only extract color now
        let current_color = {
            // Scope for mutex guard
            let mut renderer_guard = self.sphere_renderer.lock();
            renderer_guard.time += ctx.input(|i| i.stable_dt);
            renderer_guard.update_visual_state(playback_state, &self.current_audio_data);
            renderer_guard.current_color_rgb // Extract color needed for callback
                                             // point_size removed
        }; // Mutex guard drops here

        // --- Draw UI ---
        egui::CentralPanel::default().show(ctx, |ui| {
            // ... UI Code (Heading, File Path, Volume, Play/Pause, Status) ...
            // (Same as before, omitted for brevity)
            ui.heading("Audio Visualizer");
            ui.separator();
            ui.horizontal(|ui| {
                ui.label("Audio File Path (MP3):");
                ui.add_sized(
                    ui.available_size_before_wrap(),
                    egui::TextEdit::singleline(&mut self.file_path_input)
                        .hint_text("/path/to/your/audio.mp3"),
                );
            });
            ui.add_space(5.0);
            ui.horizontal(|ui| {
                ui.label("Volume:");
                let _slider_enabled = !self.is_muted;
                let volume_slider = ui.add(
                    egui::Slider::new(&mut self.volume, 0.0..=1.0)
                        .logarithmic(false)
                        .show_value(true)
                        .clamp_to_range(true)
                        .min_decimals(2),
                );
                if volume_slider.changed() {
                    self.is_muted = false;
                    self.pre_mute_volume = self.volume;
                    if let Ok(manager) = &mut self.audio_manager {
                        manager.set_output_volume(self.volume);
                    }
                }
                let mute_button_text = if self.is_muted { "Unmute" } else { "Mute" };
                if ui.button(mute_button_text).clicked() {
                    self.is_muted = !self.is_muted;
                    let new_volume;
                    if self.is_muted {
                        self.pre_mute_volume = self.volume;
                        new_volume = 0.0;
                        self.volume = 0.0;
                    } else {
                        new_volume = self.pre_mute_volume;
                        self.volume = self.pre_mute_volume;
                    }
                    if let Ok(manager) = &mut self.audio_manager {
                        manager.set_output_volume(new_volume);
                    }
                }
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
                if ui
                    .add_enabled(play_button_enabled, egui::Button::new(play_button_text))
                    .clicked()
                {
                    if let Ok(manager) = &mut self.audio_manager {
                        self.action_error_message = None;
                        let current_manager_state = manager.get_state();
                        let manager_knows_current_file = manager
                            .get_current_file_path()
                            .map_or(false, |p| p == &self.file_path_input);
                        let input_is_empty_but_manager_has_file = self.file_path_input.is_empty()
                            && manager.get_current_file_path().is_some();
                        let mut op_result: Result<(), String> = Ok(());
                        match current_manager_state {
                            PlaybackState::Playing => {
                                if manager_knows_current_file {
                                    manager.pause_playback();
                                } else {
                                    op_result = manager.load_and_play_file(
                                        &self.file_path_input,
                                        self.analysis_sender.clone(),
                                    );
                                }
                            }
                            PlaybackState::Paused => {
                                if manager_knows_current_file || input_is_empty_but_manager_has_file
                                {
                                    manager.resume_playback();
                                } else {
                                    op_result = manager.load_and_play_file(
                                        &self.file_path_input,
                                        self.analysis_sender.clone(),
                                    );
                                }
                            }
                            PlaybackState::Idle | PlaybackState::Loaded => {
                                let target_path = if self.file_path_input.is_empty()
                                    && manager.get_current_file_path().is_some()
                                {
                                    manager.get_current_file_path().unwrap().clone()
                                } else {
                                    self.file_path_input.clone()
                                };
                                if !target_path.is_empty() {
                                    op_result = manager.load_and_play_file(
                                        &target_path,
                                        self.analysis_sender.clone(),
                                    );
                                } else {
                                    op_result = Err("No file path provided".to_string());
                                }
                            }
                        }
                        if let Err(e) = op_result {
                            self.action_error_message = Some(e);
                        }
                    }
                }
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

            // --- Visualization Area ---
            ui.label("3D Point Sphere Visualization:");
            let desired_size = ui.available_size_before_wrap() * egui::vec2(1.0, 0.75);
            let (rect, _response) = ui.allocate_exact_size(desired_size, egui::Sense::hover());

            let (primitive_is_initialized, mvp_matrix_option) = {
                let renderer_guard = self.sphere_renderer.lock();
                let is_init = renderer_guard.get_primitive_arc().is_some();
                let mvp = if is_init {
                    let aspect_ratio = rect.width() / rect.height();
                    Some(renderer_guard.calculate_mvp(aspect_ratio))
                } else {
                    None
                };
                (is_init, mvp)
            };

            if primitive_is_initialized {
                if let (Some(queue_arc), Some(mvp_matrix)) = (&self.wgpu_queue, mvp_matrix_option) {
                    // Get the primitive Arc again (cheap clone)
                    let primitive_arc = self
                        .sphere_renderer
                        .lock()
                        .get_primitive_arc()
                        .expect("Primitive should be initialized here");

                    let cb = eframe::egui_wgpu::Callback::new_paint_callback(
                        rect,
                        Custom3DPaintCallback {
                            primitive: primitive_arc,
                            mvp_matrix,
                            queue: queue_arc.clone(),
                            color: current_color, // Pass color calculated above
                                                  // point_size removed
                        },
                    );
                    ui.painter().add(cb);
                } else {
                    /* WGPU Queue missing */
                    ui.painter().rect_filled(rect, 0.0, egui::Color32::DARK_RED);
                    ui.painter().text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        "WGPU Queue N/A",
                        egui::FontId::default(),
                        egui::Color32::WHITE,
                    );
                }
            } else {
                /* Renderer primitive not initialized */
                ui.painter()
                    .rect_filled(rect, 0.0, egui::Color32::DARK_GRAY);
                ui.painter().text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "Renderer N/A",
                    egui::FontId::default(),
                    egui::Color32::WHITE,
                );
            }
        }); // End CentralPanel

        ctx.request_repaint();
    }
}
