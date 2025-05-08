use crate::audio::{AudioAnalysisData, AudioManager, PlaybackState};
use crate::visualization::{
    renderer::WgpuSphereRenderer, sphere_geometry::generate_sphere_points_fibonacci,
};
use eframe::{egui, egui_wgpu::CallbackTrait, App, Frame};
use std::path::Path;
use std::sync::{mpsc, Arc};
use type_map::concurrent::TypeMap;
use wgpu; // Required for Arc<wgpu::Queue> things

const NUM_SPHERE_POINTS: usize = 2000;
const SPHERE_RADIUS: f32 = 1.0;
const DEFAULT_VOLUME: Option<f32> = Some(0.25);

// Callback stores queue obtained from App state
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
        _resources: &'a TypeMap, // Unused queue source for now
    ) {
        crate::visualization::renderer::WgpuSphereRenderer::paint_primitive(
            &self.primitive,
            &self.mvp_matrix,
            render_pass,
            // Use stored queue
            &self.queue,
        );
    }
}

pub struct AudioVisualizerApp {
    file_path_input: String,
    audio_manager: Result<AudioManager, String>,
    action_error_message: Option<String>,
    sphere_renderer: WgpuSphereRenderer,

    // TODO: may ot need `wgpu_device` anymore
    #[allow(dead_code)]
    wgpu_device: Option<Arc<wgpu::Device>>,
    // WGPU Queue state needed for callbacks
    wgpu_queue: Option<Arc<wgpu::Queue>>,

    audio_analysis_receiver: mpsc::Receiver<AudioAnalysisData>,
    analysis_sender: mpsc::SyncSender<AudioAnalysisData>,
    current_audio_data: Option<AudioAnalysisData>,

    volume: f32,
    pre_mute_volume: f32,
    is_muted: bool,
}

impl AudioVisualizerApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let sphere_points = generate_sphere_points_fibonacci(SPHERE_RADIUS, NUM_SPHERE_POINTS);
        let mut sphere_renderer = WgpuSphereRenderer::new(sphere_points);

        let mut app_wgpu_device_arc = None;
        let mut app_wgpu_queue_arc = None;

        // Initialize WGPU resources for the renderer
        if let Some(wgpu_render_state) = &cc.wgpu_render_state {
            let device_arc = wgpu_render_state.device.clone();
            let queue_arc = wgpu_render_state.queue.clone();
            let target_format = wgpu_render_state.target_format;

            if let Err(e) = sphere_renderer.prepare(&device_arc, target_format) {
                // TODO: This is fine as long as we don't mess with the
                //   render_pipeline (depth_stencil testing)
                tracing::error!("Failed to prepare WGPU sphere renderer: {}", e);
            } else {
                app_wgpu_device_arc = Some(device_arc);
                app_wgpu_queue_arc = Some(queue_arc);
            }
        } else {
            tracing::warn!(
                "WGPU render state not available at creation. Visualization might not work."
            );
        }

        // Create bounded channel to receive derived meta info from audio analysis
        // Bounded channel will prevent excessive memory usage if UI lags
        let (analysis_sender, audio_analysis_receiver) = mpsc::sync_channel(10);
        let audio_manager = AudioManager::new(DEFAULT_VOLUME);

        Self {
            file_path_input: "/Users/donald/Downloads/example.mp3".to_string(),
            audio_manager,
            action_error_message: None,
            sphere_renderer,
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
        // Update Audio State
        if let Ok(manager) = &mut self.audio_manager {
            manager.check_and_update_finished_state();
        }

        // Receive Audio Analysis Data
        while let Ok(data) = self.audio_analysis_receiver.try_recv() {
            // Store the latest data received this frame
            // If no data received this frame, self.current_audio_data retains the previous value.
            // TODO: Consider adding logic to fade out effect if no data received for a while?
            self.current_audio_data = Some(data);
        }

        // Update the renderer with the latest audio data
        self.sphere_renderer.time += ctx.input(|i| i.stable_dt);
        self.sphere_renderer
            .update_with_audio(&self.current_audio_data);

        // Build / Render UI
        // TODO  break this UI stuff up into a separate functions
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Audio Visualizer");
            ui.separator();

            // Audio Controls
            ui.horizontal(|ui| {
                // Audio File Path input
                ui.label("Audio File Path (MP3):");
                ui.add_sized(
                    ui.available_size_before_wrap(),
                    egui::TextEdit::singleline(&mut self.file_path_input)
                        .hint_text("/path/to/your/audio.mp3"),
                );
            });

            ui.add_space(5.0);

            // Volume Slider
            ui.horizontal(|ui| {
                let volume_slider = ui.add_enabled(
                    true,
                    egui::Slider::new(&mut self.volume, 0.0..=1.0)
                        .logarithmic(false)
                        .show_value(true)
                        .clamp_to_range(true)
                        .min_decimals(2)
                        .text("Volume"),
                );

                // Update audio manager on slider value change
                if volume_slider.changed() {
                    self.is_muted = false;
                    self.pre_mute_volume = self.volume;
                    if let Ok(manager) = &mut self.audio_manager {
                        manager.set_output_volume(self.volume);
                    }
                }

                // Mute Button
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

            // Play / Pause Button
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
                                // If playing current file -> Pause, else Play New
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
                                // If paused current file -> Resume, else Play New
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
                                // Play new or play loaded
                                // Check if trying to play the currently loaded file (if any) or a new one
                                let target_path = if self.file_path_input.is_empty()
                                    && manager.get_current_file_path().is_some()
                                {
                                    manager.get_current_file_path().unwrap().clone()
                                // Use loaded path if input path is empty
                                } else {
                                    // Use file input path
                                    self.file_path_input.clone()
                                };
                                // Play the target file
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

            // Point Cloud / Visualization Area
            ui.label("3D Point Sphere Visualization:");
            let desired_size = ui.available_size_before_wrap() * egui::vec2(1.0, 0.75);
            let (rect, _response) = ui.allocate_exact_size(desired_size, egui::Sense::hover());

            if let Some(primitive_arc) = self.sphere_renderer.get_primitive_arc() {
                if let Some(queue_arc) = &self.wgpu_queue {
                    let aspect_ratio = rect.width() / rect.height();
                    // Calculate MVP *after* update_with_audio has potentially changed scale
                    let mvp_matrix = self.sphere_renderer.calculate_mvp(aspect_ratio);

                    let cb = eframe::egui_wgpu::Callback::new_paint_callback(
                        rect,
                        Custom3DPaintCallback {
                            primitive: primitive_arc,
                            mvp_matrix,
                            queue: queue_arc.clone(), // Clone Arc for the callback
                        },
                    );
                    ui.painter().add(cb);
                } else {
                    ui.painter().rect_filled(rect, 0.0, egui::Color32::DARK_RED);
                    ui.painter().text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        "WGPU Queue not available",
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

        // TODO: Determine if repaint request interval is needed if we do some heavy things later.
        // Request repaint continuously for animation and audio updates
        ctx.request_repaint();
    }
}
