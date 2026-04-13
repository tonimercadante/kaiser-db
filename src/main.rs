use anyhow::Context;
use glyphon::{
    Attrs, Buffer, Cache, Color, Family, FontSystem, Metrics, Resolution, Shaping, SwashCache,
    TextArea, TextAtlas, TextBounds, TextRenderer, Viewport,
};
use std::sync::{Arc, mpsc};
use wgpu::MultisampleState;
use wgpu::util::DeviceExt;
use winit::{
    application::ApplicationHandler,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, EventLoop},
    keyboard::ModifiersState,
    window::Window,
};

// SQLX
use sqlx::postgres::{PgPool, PgPoolOptions};
use sqlx::types::chrono::{DateTime, NaiveDate, NaiveDateTime, NaiveTime, Utc};
use sqlx::types::time::OffsetDateTime;
use sqlx::types::{BigDecimal, JsonValue, Uuid};
use sqlx::{Column, Row, TypeInfo};

const OUTER_PADDING: f32 = 20.0;
const PANEL_GAP: f32 = 12.0;
const QUERY_TEXT_PADDING: f32 = 12.0;
const TABLE_CELL_PADDING_X: f32 = 10.0;
const TABLE_CELL_PADDING_Y: f32 = 6.0;
const TABLE_ROW_HEIGHT: f32 = 34.0;
const TABLE_MIN_COL_WIDTH: f32 = 120.0;
const TABLE_MAX_COL_WIDTH: f32 = 720.0;
const TABLE_SCROLLBAR_MARGIN: f32 = 8.0;
const TABLE_SCROLLBAR_HEIGHT: f32 = 10.0;
const TABLE_SCROLLBAR_MIN_THUMB_WIDTH: f32 = 48.0;
const QUERY_MIN_HEIGHT: f32 = 96.0;
const QUERY_MAX_HEIGHT: f32 = 220.0;
const TABLE_HEADER_COLOR: [f32; 4] = [0.24, 0.24, 0.28, 1.0];
const TABLE_ROW_COLOR: [f32; 4] = [0.12, 0.12, 0.12, 1.0];
const QUERY_PANEL_COLOR: [f32; 4] = [0.15, 0.15, 0.15, 1.0];
const TABLE_PANEL_COLOR: [f32; 4] = [0.1, 0.1, 0.1, 1.0];
const TABLE_SCROLLBAR_TRACK_COLOR: [f32; 4] = [0.18, 0.18, 0.18, 1.0];
const TABLE_SCROLLBAR_THUMB_COLOR: [f32; 4] = [0.36, 0.36, 0.44, 1.0];

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct RectInstance {
    position: [f32; 2],
    size: [f32; 2],
    color: [f32; 4],
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct Globals {
    resolution: [f32; 2],
    _pad: [f32; 2],
}

#[derive(Clone, Copy)]
struct UiLayout {
    query_x: f32,
    query_y: f32,
    query_w: f32,
    query_h: f32,
    table_x: f32,
    table_y: f32,
    table_w: f32,
    table_h: f32,
}

#[derive(Clone, Copy)]
struct ScrollbarGeometry {
    track_x: f32,
    track_y: f32,
    track_w: f32,
    track_h: f32,
    thumb_x: f32,
    thumb_w: f32,
}

fn clamp(value: f32, min: f32, max: f32) -> f32 {
    value.max(min).min(max)
}

fn compute_layout(w: f32, h: f32) -> UiLayout {
    let content_w = (w - OUTER_PADDING * 2.0).max(160.0);
    let available_h = (h - OUTER_PADDING * 2.0 - PANEL_GAP).max(180.0);
    let query_h = clamp(available_h * 0.28, QUERY_MIN_HEIGHT, QUERY_MAX_HEIGHT)
        .min((available_h - 80.0).max(60.0));
    let table_h = (available_h - query_h).max(60.0);

    UiLayout {
        query_x: OUTER_PADDING,
        query_y: OUTER_PADDING,
        query_w: content_w,
        query_h,
        table_x: OUTER_PADDING,
        table_y: OUTER_PADDING + query_h + PANEL_GAP,
        table_w: content_w,
        table_h,
    }
}

fn build_rects(
    layout: UiLayout,
    table_viewport_h: f32,
    column_widths: &[f32],
    rows: usize,
    scroll_x: f32,
    scroll_y: f32,
    horizontal_scrollbar: Option<ScrollbarGeometry>,
) -> Vec<RectInstance> {
    let mut rects = vec![
        RectInstance {
            position: [layout.query_x, layout.query_y],
            size: [layout.query_w, layout.query_h],
            color: QUERY_PANEL_COLOR,
        },
        RectInstance {
            position: [layout.table_x, layout.table_y],
            size: [layout.table_w, layout.table_h],
            color: TABLE_PANEL_COLOR,
        },
    ];

    let rows = rows.max(1);
    let y = layout.table_y - scroll_y;
    let viewport_left = layout.table_x;
    let viewport_right = layout.table_x + layout.table_w;
    let viewport_top = layout.table_y;
    let viewport_bottom = layout.table_y + table_viewport_h;

    for r in 0..rows {
        let row_y = y + r as f32 * TABLE_ROW_HEIGHT;
        let row_bottom = row_y + TABLE_ROW_HEIGHT;

        if row_bottom < viewport_top {
            continue;
        }

        if row_y > viewport_bottom {
            break;
        }

        let mut x = layout.table_x - scroll_x;

        for width in column_widths {
            let cell_left = x;
            let cell_right = x + *width - 1.0;
            let visible_left = cell_left.max(viewport_left);
            let visible_right = cell_right.min(viewport_right);
            let visible_top = row_y.max(viewport_top);
            let visible_bottom = row_bottom.min(viewport_bottom);

            if visible_right > visible_left && visible_bottom > visible_top {
                rects.push(RectInstance {
                    position: [visible_left, visible_top],
                    size: [
                        (visible_right - visible_left).max(1.0),
                        (visible_bottom - visible_top).max(1.0),
                    ],
                    color: if r == 0 {
                        TABLE_HEADER_COLOR
                    } else {
                        TABLE_ROW_COLOR
                    },
                });
            }
            x += *width;
        }
    }

    if let Some(scrollbar) = horizontal_scrollbar {
        rects.push(RectInstance {
            position: [scrollbar.track_x, scrollbar.track_y],
            size: [scrollbar.track_w, scrollbar.track_h],
            color: TABLE_SCROLLBAR_TRACK_COLOR,
        });
        rects.push(RectInstance {
            position: [scrollbar.thumb_x, scrollbar.track_y],
            size: [scrollbar.thumb_w, scrollbar.track_h],
            color: TABLE_SCROLLBAR_THUMB_COLOR,
        });
    }

    rects
}

fn format_cell(row: &sqlx::postgres::PgRow, index: usize) -> String {
    if let Ok(value) = row.try_get::<Option<String>, _>(index) {
        return value.unwrap_or_else(|| "NULL".to_string());
    }

    if let Ok(value) = row.try_get::<Option<&str>, _>(index) {
        return value
            .map(|v| v.to_string())
            .unwrap_or_else(|| "NULL".to_string());
    }

    if let Ok(value) = row.try_get::<Option<Uuid>, _>(index) {
        return value
            .map(|v| v.to_string())
            .unwrap_or_else(|| "NULL".to_string());
    }

    if let Ok(value) = row.try_get::<Option<NaiveDateTime>, _>(index) {
        return value
            .map(|v| v.to_string())
            .unwrap_or_else(|| "NULL".to_string());
    }

    if let Ok(value) = row.try_get::<Option<DateTime<Utc>>, _>(index) {
        return value
            .map(|v| v.to_rfc3339())
            .unwrap_or_else(|| "NULL".to_string());
    }

    if let Ok(value) = row.try_get::<Option<OffsetDateTime>, _>(index) {
        return value
            .map(|v| v.to_string())
            .unwrap_or_else(|| "NULL".to_string());
    }

    if let Ok(value) = row.try_get::<Option<NaiveDate>, _>(index) {
        return value
            .map(|v| v.to_string())
            .unwrap_or_else(|| "NULL".to_string());
    }

    if let Ok(value) = row.try_get::<Option<NaiveTime>, _>(index) {
        return value
            .map(|v| v.to_string())
            .unwrap_or_else(|| "NULL".to_string());
    }

    if let Ok(value) = row.try_get::<Option<i32>, _>(index) {
        return value
            .map(|v| v.to_string())
            .unwrap_or_else(|| "NULL".to_string());
    }

    if let Ok(value) = row.try_get::<Option<i16>, _>(index) {
        return value
            .map(|v| v.to_string())
            .unwrap_or_else(|| "NULL".to_string());
    }

    if let Ok(value) = row.try_get::<Option<i64>, _>(index) {
        return value
            .map(|v| v.to_string())
            .unwrap_or_else(|| "NULL".to_string());
    }

    if let Ok(value) = row.try_get::<Option<f32>, _>(index) {
        return value
            .map(|v| v.to_string())
            .unwrap_or_else(|| "NULL".to_string());
    }

    if let Ok(value) = row.try_get::<Option<f64>, _>(index) {
        return value
            .map(|v| v.to_string())
            .unwrap_or_else(|| "NULL".to_string());
    }

    if let Ok(value) = row.try_get::<Option<BigDecimal>, _>(index) {
        return value
            .map(|v| v.normalized().to_string())
            .unwrap_or_else(|| "NULL".to_string());
    }

    if let Ok(value) = row.try_get::<Option<bool>, _>(index) {
        return value
            .map(|v| v.to_string())
            .unwrap_or_else(|| "NULL".to_string());
    }

    if let Ok(value) = row.try_get::<Option<JsonValue>, _>(index) {
        return value
            .map(|v| v.to_string())
            .unwrap_or_else(|| "NULL".to_string());
    }

    if let Ok(value) = row.try_get::<Option<Vec<u8>>, _>(index) {
        return value
            .map(|bytes| {
                if bytes.is_empty() {
                    "[]".to_string()
                } else {
                    bytes
                        .iter()
                        .map(|byte| format!("{byte:02x}"))
                        .collect::<Vec<_>>()
                        .join("")
                }
            })
            .unwrap_or_else(|| "NULL".to_string());
    }

    let type_name = row
        .columns()
        .get(index)
        .map(|column| column.type_info().name().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    format!("<unsupported:{type_name}>")
}

fn build_table_buffers(font_system: &mut FontSystem, table_data: &[Vec<String>]) -> Vec<Buffer> {
    let mut table_buffers = Vec::new();

    for row in table_data {
        for cell in row {
            let mut buf = Buffer::new(font_system, Metrics::new(16.0, 20.0));
            buf.set_size(
                font_system,
                Some(TABLE_MIN_COL_WIDTH),
                Some(TABLE_ROW_HEIGHT),
            );
            buf.set_text(
                font_system,
                cell,
                &Attrs::new().family(Family::Monospace),
                Shaping::Advanced,
                None,
            );
            buf.shape_until_scroll(font_system, false);
            table_buffers.push(buf);
        }
    }

    table_buffers
}

enum QueryUiMessage {
    Table(Vec<Vec<String>>),
}

pub struct App {
    state: Option<State>,
    pool: PgPool,
    modifiers: ModifiersState,
    query_results_rx: mpsc::Receiver<QueryUiMessage>,
    query_results_tx: mpsc::Sender<QueryUiMessage>,
}

impl App {
    fn new(pool: PgPool) -> Self {
        let (query_results_tx, query_results_rx) = mpsc::channel();
        Self {
            state: None,
            pool,
            modifiers: ModifiersState::default(),
            query_results_rx,
            query_results_tx,
        }
    }
}
pub struct State {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,

    // Text glyphon stuff
    font_system: FontSystem,
    swash_cache: SwashCache,
    viewport: Viewport,
    atlas: glyphon::TextAtlas,
    text_renderer: glyphon::TextRenderer,
    text_buffer: glyphon::Buffer,
    table_buffers: Vec<glyphon::Buffer>,

    // Shaders stuff
    _globals_buffer: wgpu::Buffer,
    globals_bind_group: wgpu::BindGroup,
    rect_buffer: wgpu::Buffer,
    rect_buffer_capacity: usize,
    rect_count: usize,
    rect_pipeline: wgpu::RenderPipeline,

    // UI stuff
    query: String,
    table_data: Vec<Vec<String>>,
    column_widths: Vec<f32>,
    scroll_x: f32,
    scroll_y: f32,
    cursor_position: (f32, f32),
    horizontal_scrollbar_drag: Option<f32>,

    // window has to be last because a bug or something
    window: Arc<Window>,
}

impl State {
    pub async fn new(window: Arc<Window>) -> anyhow::Result<Self> {
        let instance = wgpu::Instance::default();
        let surface = instance.create_surface(window.clone())?;

        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                compatible_surface: Some(&surface),
                ..Default::default()
            })
            .await?;

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor::default())
            .await?;

        let size = window.inner_size();
        let config = surface
            .get_default_config(&adapter, size.width, size.height)
            .ok_or(anyhow::anyhow!("no surface config"))?;

        surface.configure(&device, &config);

        // TExt glyphon stuff
        let mut font_system = FontSystem::new();
        let swash_cache = SwashCache::new();
        let cache = Cache::new(&device);
        let viewport = Viewport::new(&device, &cache);
        let mut atlas = TextAtlas::new(&device, &queue, &cache, config.format);
        let text_renderer =
            TextRenderer::new(&mut atlas, &device, MultisampleState::default(), None);

        let mut text_buffer = Buffer::new(&mut font_system, Metrics::new(30.0, 42.0));
        let layout = compute_layout(size.width as f32, size.height as f32);
        text_buffer.set_size(
            &mut font_system,
            Some((layout.query_w - QUERY_TEXT_PADDING * 2.0).max(40.0)),
            Some((layout.query_h - QUERY_TEXT_PADDING * 2.0).max(40.0)),
        );

        let query = String::from("SELECT *, magnus FROM kaiser;");

        text_buffer.set_text(
            &mut font_system,
            &query,
            &Attrs::new().family(Family::SansSerif),
            Shaping::Advanced,
            None,
        );
        text_buffer.shape_until_scroll(&mut font_system, false);

        let table_data = vec![
            vec!["id".to_string(), "name".to_string(), "email".to_string()],
            vec![
                "1".to_string(),
                "alice".to_string(),
                "alice@example.com".to_string(),
            ],
            vec![
                "2".to_string(),
                "bob".to_string(),
                "bob@example.com".to_string(),
            ],
            vec![
                "3".to_string(),
                "carol".to_string(),
                "carol@example.com".to_string(),
            ],
            vec![
                "4".to_string(),
                "dave".to_string(),
                "dave@example.com".to_string(),
            ],
        ];
        let table_buffers = build_table_buffers(&mut font_system, &table_data);
        let column_widths = compute_column_widths(&table_data);
        // Shader stuff
        //  // Globals uniform
        let globals = Globals {
            resolution: [size.width as f32, size.height as f32],
            _pad: [0.0; 2],
        };
        let globals_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("globals"),
            contents: bytemuck::cast_slice(&[globals]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let globals_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let globals_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &globals_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: globals_buffer.as_entire_binding(),
            }],
        });

        // Rect buffer
        let rects = build_rects(
            layout,
            compute_table_viewport_height(layout, column_widths.iter().sum()),
            &column_widths,
            table_data.len(),
            0.0,
            0.0,
            compute_horizontal_scrollbar(layout, column_widths.iter().sum(), 0.0),
        );
        let rect_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("rects"),
            size: (std::mem::size_of::<RectInstance>() * rects.len().max(1)) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&rect_buffer, 0, bytemuck::cast_slice(&rects));
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders.wgsl").into()),
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[&globals_bgl],
            immediate_size: 0,
        });

        let rect_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: None,
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<RectInstance>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &wgpu::vertex_attr_array![
                        0 => Float32x2,
                        1 => Float32x2,
                        2 => Float32x4,
                    ],
                }],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: config.format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multiview_mask: None,
            multisample: wgpu::MultisampleState::default(),
            cache: None,
        });

        Ok(Self {
            surface,
            device,
            queue,
            config,

            font_system,
            swash_cache,
            viewport,
            atlas,
            text_renderer,
            text_buffer,
            table_buffers,

            // Shader stuff
            rect_pipeline,
            _globals_buffer: globals_buffer,
            globals_bind_group,
            rect_buffer,
            rect_buffer_capacity: rects.len().max(1),
            rect_count: rects.len(),

            // UI stuff
            query,
            table_data,
            column_widths,
            scroll_x: 0.0,
            scroll_y: 0.0,
            cursor_position: (0.0, 0.0),
            horizontal_scrollbar_drag: None,

            window,
        })
    }

    fn set_table_data(&mut self, table_data: Vec<Vec<String>>) {
        self.table_buffers = build_table_buffers(&mut self.font_system, &table_data);
        self.column_widths = compute_column_widths(&table_data);
        self.table_data = table_data;
        self.clamp_scroll();
        self.update_table_buffer_sizes();
        self.sync_table_rects();
        self.window.request_redraw();
    }

    fn sync_table_rects(&mut self) {
        let rows = self.table_data.len().max(1);
        let layout = self.layout();
        let rects = build_rects(
            layout,
            self.table_viewport_height(),
            &self.column_widths,
            rows,
            self.scroll_x,
            self.scroll_y,
            self.horizontal_scrollbar_geometry(),
        );
        self.ensure_rect_buffer_capacity(rects.len());
        self.rect_count = rects.len();

        self.queue
            .write_buffer(&self.rect_buffer, 0, bytemuck::cast_slice(&rects));
    }

    fn ensure_rect_buffer_capacity(&mut self, required_rects: usize) {
        if required_rects <= self.rect_buffer_capacity {
            return;
        }

        self.rect_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("rects"),
            size: (std::mem::size_of::<RectInstance>() * required_rects.max(1)) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.rect_buffer_capacity = required_rects.max(1);
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if width > 0 && height > 0 {
            self.config.width = width;
            self.config.height = height;
            self.surface.configure(&self.device, &self.config);
            let globals = Globals {
                resolution: [width as f32, height as f32],
                _pad: [0.0; 2],
            };
            self.queue
                .write_buffer(&self._globals_buffer, 0, bytemuck::cast_slice(&[globals]));
            let layout = self.layout();
            self.text_buffer.set_size(
                &mut self.font_system,
                Some((layout.query_w - QUERY_TEXT_PADDING * 2.0).max(40.0)),
                Some((layout.query_h - QUERY_TEXT_PADDING * 2.0).max(40.0)),
            );
            self.text_buffer
                .shape_until_scroll(&mut self.font_system, false);
            self.clamp_scroll();
            self.update_table_buffer_sizes();
            self.sync_table_rects();
        }
    }

    pub fn render(&mut self) {
        let frame = match self.surface.get_current_texture() {
            Ok(frame) => frame,
            Err(wgpu::SurfaceError::Outdated) | Err(wgpu::SurfaceError::Lost) => {
                self.surface.configure(&self.device, &self.config);
                self.window.request_redraw();
                return;
            }
            Err(e) => panic!("Surface error: {e}"),
        };

        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());

        self.viewport.update(
            &self.queue,
            Resolution {
                width: self.config.width,
                height: self.config.height,
            },
        );
        let layout = self.layout();
        let table_viewport_h = self.table_viewport_height();
        let cols = self.column_widths.len().max(1);

        let mut text_areas: Vec<TextArea> = vec![TextArea {
            buffer: &self.text_buffer,
            left: layout.query_x + QUERY_TEXT_PADDING,
            top: layout.query_y + QUERY_TEXT_PADDING,
            scale: 1.0,
            bounds: TextBounds {
                left: layout.query_x as i32,
                top: layout.query_y as i32,
                right: (layout.query_x + layout.query_w) as i32,
                bottom: (layout.query_y + layout.query_h) as i32,
            },
            default_color: Color::rgb(255, 255, 255),
            custom_glyphs: &[],
        }];

        for (i, buf) in self.table_buffers.iter().enumerate() {
            let r = i / cols;
            let c = i % cols;
            let x = layout.table_x + self.column_widths.iter().take(c).sum::<f32>() - self.scroll_x;
            let y = layout.table_y + r as f32 * TABLE_ROW_HEIGHT - self.scroll_y;
            text_areas.push(TextArea {
                buffer: buf,
                left: x + TABLE_CELL_PADDING_X,
                top: y + TABLE_CELL_PADDING_Y,
                scale: 1.0,
                bounds: TextBounds {
                    left: layout.table_x as i32,
                    top: layout.table_y as i32,
                    right: (layout.table_x + layout.table_w) as i32,
                    bottom: (layout.table_y + table_viewport_h) as i32,
                },
                default_color: if r == 0 {
                    Color::rgb(180, 180, 255)
                } else {
                    Color::rgb(255, 255, 255)
                },
                custom_glyphs: &[],
            });
        }

        self.text_renderer
            .prepare(
                &self.device,
                &self.queue,
                &mut self.font_system,
                &mut self.atlas,
                &self.viewport,
                text_areas,
                &mut self.swash_cache,
            )
            .unwrap();

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.05,
                            g: 0.05,
                            b: 1.00,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });

            pass.set_pipeline(&self.rect_pipeline);
            pass.set_bind_group(0, &self.globals_bind_group, &[]);
            pass.set_vertex_buffer(0, self.rect_buffer.slice(..));
            pass.draw(0..6, 0..self.rect_count as u32);

            self.text_renderer
                .render(&self.atlas, &self.viewport, &mut pass)
                .unwrap();
        }

        self.queue.submit([encoder.finish()]);
        frame.present();
        self.atlas.trim();
    }

    fn layout(&self) -> UiLayout {
        compute_layout(self.config.width as f32, self.config.height as f32)
    }

    fn table_content_width(&self) -> f32 {
        self.column_widths.iter().sum::<f32>().max(1.0)
    }

    fn table_content_height(&self) -> f32 {
        (self.table_data.len().max(1) as f32) * TABLE_ROW_HEIGHT
    }

    fn max_scroll_x(&self) -> f32 {
        let layout = self.layout();
        (self.table_content_width() - layout.table_w).max(0.0)
    }

    fn max_scroll_y(&self) -> f32 {
        (self.table_content_height() - self.table_viewport_height()).max(0.0)
    }

    fn clamp_scroll(&mut self) {
        self.scroll_x = self.scroll_x.clamp(0.0, self.max_scroll_x());
        self.scroll_y = self.scroll_y.clamp(0.0, self.max_scroll_y());
    }

    fn scroll_table(&mut self, delta_x: f32, delta_y: f32) {
        if delta_x == 0.0 && delta_y == 0.0 {
            return;
        }

        self.scroll_x = (self.scroll_x + delta_x).clamp(0.0, self.max_scroll_x());
        self.scroll_y = (self.scroll_y + delta_y).clamp(0.0, self.max_scroll_y());
        self.sync_table_rects();
        self.window.request_redraw();
    }

    fn table_viewport_height(&self) -> f32 {
        compute_table_viewport_height(self.layout(), self.table_content_width())
    }

    fn horizontal_scrollbar_geometry(&self) -> Option<ScrollbarGeometry> {
        compute_horizontal_scrollbar(self.layout(), self.table_content_width(), self.scroll_x)
    }

    fn set_scroll_x_from_thumb(&mut self, thumb_x: f32) {
        let Some(scrollbar) = self.horizontal_scrollbar_geometry() else {
            return;
        };

        let travel = (scrollbar.track_w - scrollbar.thumb_w).max(0.0);
        if travel <= 0.0 {
            self.scroll_x = 0.0;
        } else {
            let clamped_thumb_x = thumb_x.clamp(scrollbar.track_x, scrollbar.track_x + travel);
            let ratio = (clamped_thumb_x - scrollbar.track_x) / travel;
            self.scroll_x = ratio * self.max_scroll_x();
        }

        self.sync_table_rects();
        self.window.request_redraw();
    }

    fn handle_horizontal_scrollbar_press(&mut self, x: f32, y: f32) -> bool {
        let Some(scrollbar) = self.horizontal_scrollbar_geometry() else {
            return false;
        };

        if point_in_rect(
            x,
            y,
            scrollbar.thumb_x,
            scrollbar.track_y,
            scrollbar.thumb_w,
            scrollbar.track_h,
        ) {
            self.horizontal_scrollbar_drag = Some(x - scrollbar.thumb_x);
            return true;
        }

        if point_in_rect(
            x,
            y,
            scrollbar.track_x,
            scrollbar.track_y,
            scrollbar.track_w,
            scrollbar.track_h,
        ) {
            let centered_thumb_x = x - scrollbar.thumb_w * 0.5;
            self.set_scroll_x_from_thumb(centered_thumb_x);
            self.horizontal_scrollbar_drag = Some(scrollbar.thumb_w * 0.5);
            return true;
        }

        false
    }

    fn update_table_buffer_sizes(&mut self) {
        let col_count = self.column_widths.len().max(1);
        let cell_count = self.table_buffers.len();
        if cell_count == 0 {
            return;
        }

        for (i, buf) in self.table_buffers.iter_mut().enumerate() {
            let col = i % col_count;
            let width = self
                .column_widths
                .get(col)
                .copied()
                .unwrap_or(TABLE_MIN_COL_WIDTH);
            buf.set_size(
                &mut self.font_system,
                Some((width - TABLE_CELL_PADDING_X * 2.0).max(16.0)),
                Some((TABLE_ROW_HEIGHT - TABLE_CELL_PADDING_Y * 2.0).max(16.0)),
            );
            buf.shape_until_scroll(&mut self.font_system, false);
        }
    }
}

fn compute_column_widths(table_data: &[Vec<String>]) -> Vec<f32> {
    let cols = table_data.first().map(|row| row.len()).unwrap_or(1).max(1);
    let mut widths = vec![TABLE_MIN_COL_WIDTH; cols];

    for row in table_data {
        for (index, cell) in row.iter().enumerate() {
            let char_width = if index == 0 { 12.0 } else { 14.0 };
            let estimate = cell.chars().count() as f32 * char_width + TABLE_CELL_PADDING_X * 2.0;
            widths[index] = widths[index].max(estimate.min(TABLE_MAX_COL_WIDTH));
        }
    }

    widths
}

fn compute_table_viewport_height(layout: UiLayout, content_width: f32) -> f32 {
    let reserve = if content_width > layout.table_w {
        TABLE_SCROLLBAR_HEIGHT + TABLE_SCROLLBAR_MARGIN * 2.0
    } else {
        0.0
    };

    (layout.table_h - reserve).max(40.0)
}

fn compute_horizontal_scrollbar(
    layout: UiLayout,
    content_width: f32,
    scroll_x: f32,
) -> Option<ScrollbarGeometry> {
    if content_width <= layout.table_w {
        return None;
    }

    let track_x = layout.table_x + TABLE_SCROLLBAR_MARGIN;
    let track_w = (layout.table_w - TABLE_SCROLLBAR_MARGIN * 2.0).max(40.0);
    let track_h = TABLE_SCROLLBAR_HEIGHT;
    let track_y = layout.table_y + layout.table_h - TABLE_SCROLLBAR_MARGIN - track_h;
    let visible_ratio = (layout.table_w / content_width).clamp(0.0, 1.0);
    let thumb_w = (track_w * visible_ratio).clamp(TABLE_SCROLLBAR_MIN_THUMB_WIDTH, track_w);
    let travel = (track_w - thumb_w).max(0.0);
    let max_scroll_x = (content_width - layout.table_w).max(0.0);
    let thumb_x = if max_scroll_x > 0.0 {
        track_x + (scroll_x / max_scroll_x) * travel
    } else {
        track_x
    };

    Some(ScrollbarGeometry {
        track_x,
        track_y,
        track_w,
        track_h,
        thumb_x,
        thumb_w,
    })
}

fn point_in_rect(px: f32, py: f32, x: f32, y: f32, w: f32, h: f32) -> bool {
    px >= x && px <= x + w && py >= y && py <= y + h
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let window = Arc::new(
            event_loop
                .create_window(Window::default_attributes().with_title("Kaiser db"))
                .unwrap(),
        );
        self.state = Some(pollster::block_on(State::new(window)).unwrap());
        self.state.as_ref().unwrap().window.request_redraw();
    }

    fn window_event(
        &mut self,
        event_loop: &winit::event_loop::ActiveEventLoop,
        _window_id: winit::window::WindowId,
        event: winit::event::WindowEvent,
    ) {
        let mut pending_messages = Vec::new();
        while let Ok(message) = self.query_results_rx.try_recv() {
            pending_messages.push(message);
        }

        let state = match &mut self.state {
            Some(s) => s,
            None => return,
        };

        for message in pending_messages {
            match message {
                QueryUiMessage::Table(table_data) => state.set_table_data(table_data),
            }
        }

        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                state.resize(size.width, size.height);
                state.render();
            }
            WindowEvent::RedrawRequested => state.render(),
            WindowEvent::ModifiersChanged(modifiers) => {
                self.modifiers = modifiers.state();
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state == winit::event::ElementState::Pressed {
                    match &event.logical_key {
                        winit::keyboard::Key::Character(c)
                            if self.modifiers.super_key() && c.eq_ignore_ascii_case("r") =>
                        {
                            let pool = self.pool.clone();
                            let query = state.query.clone();
                            let window = state.window.clone();
                            let query_results_tx = self.query_results_tx.clone();
                            tokio::spawn(async move {
                                println!("Executing query...");

                                match sqlx::query(&query).fetch_all(&pool).await {
                                    Ok(rows) => {
                                        if rows.is_empty() {
                                            println!("Query returned 0 rows.");
                                            let _ =
                                                query_results_tx.send(QueryUiMessage::Table(vec![
                                                    vec!["Query returned 0 rows.".to_string()],
                                                ]));
                                            window.request_redraw();
                                            return;
                                        }

                                        let columns = rows[0]
                                            .columns()
                                            .iter()
                                            .map(|column| column.name().to_string())
                                            .collect::<Vec<_>>();
                                        println!("Columns: {}", columns.join(" | "));

                                        let mut table_data = vec![columns.clone()];

                                        for row in rows {
                                            let values = row
                                                .columns()
                                                .iter()
                                                .enumerate()
                                                .map(|(index, _)| format_cell(&row, index))
                                                .collect::<Vec<_>>();

                                            println!("{}", values.join(" | "));
                                            table_data.push(values);
                                        }

                                        let _ = query_results_tx
                                            .send(QueryUiMessage::Table(table_data));
                                        window.request_redraw();
                                    }
                                    Err(fetch_error) => match sqlx::query(&query)
                                        .execute(&pool)
                                        .await
                                    {
                                        Ok(result) => {
                                            println!(
                                                "Query executed successfully. Rows affected: {}",
                                                result.rows_affected()
                                            );
                                            let _ =
                                                query_results_tx.send(QueryUiMessage::Table(vec![
                                                    vec![format!(
                                                        "Rows affected: {}",
                                                        result.rows_affected()
                                                    )],
                                                ]));
                                            window.request_redraw();
                                        }
                                        Err(execute_error) => {
                                            eprintln!("Failed to fetch rows: {fetch_error}");
                                            eprintln!("Failed to execute query: {execute_error}");
                                            let _ =
                                                query_results_tx.send(QueryUiMessage::Table(vec![
                                                    vec![format!("Query failed: {execute_error}")],
                                                ]));
                                            window.request_redraw();
                                        }
                                    },
                                }
                            });
                        }
                        winit::keyboard::Key::Character(c) => state.query.push_str(c),
                        winit::keyboard::Key::Named(winit::keyboard::NamedKey::Space) => {
                            state.query.push(' ');
                        }
                        winit::keyboard::Key::Named(winit::keyboard::NamedKey::Backspace) => {
                            state.query.pop();
                        }
                        winit::keyboard::Key::Named(winit::keyboard::NamedKey::Enter) => {
                            state.query.push('\n');
                        }
                        winit::keyboard::Key::Named(winit::keyboard::NamedKey::ArrowDown) => {
                            state.scroll_table(0.0, TABLE_ROW_HEIGHT);
                        }
                        winit::keyboard::Key::Named(winit::keyboard::NamedKey::ArrowUp) => {
                            state.scroll_table(0.0, -TABLE_ROW_HEIGHT);
                        }
                        winit::keyboard::Key::Named(winit::keyboard::NamedKey::ArrowRight) => {
                            state.scroll_table(TABLE_MIN_COL_WIDTH * 0.5, 0.0);
                        }
                        winit::keyboard::Key::Named(winit::keyboard::NamedKey::ArrowLeft) => {
                            state.scroll_table(-TABLE_MIN_COL_WIDTH * 0.5, 0.0);
                        }
                        _ => {}
                    }
                    state.text_buffer.set_text(
                        &mut state.font_system,
                        &state.query,
                        &Attrs::new().family(Family::SansSerif),
                        Shaping::Advanced,
                        None,
                    );
                    state
                        .text_buffer
                        .shape_until_scroll(&mut state.font_system, false);
                    state.window.request_redraw();
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let (dx, dy) = match delta {
                    winit::event::MouseScrollDelta::LineDelta(x, y) => {
                        (x * TABLE_MIN_COL_WIDTH * 0.75, -y * TABLE_ROW_HEIGHT)
                    }
                    winit::event::MouseScrollDelta::PixelDelta(pos) => (pos.x as f32, pos.y as f32),
                };

                if self.modifiers.shift_key() {
                    state.scroll_table(-dy, 0.0);
                } else {
                    state.scroll_table(-dx, -dy);
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                state.cursor_position = (position.x as f32, position.y as f32);
                if let Some(grab_offset) = state.horizontal_scrollbar_drag {
                    state.set_scroll_x_from_thumb(state.cursor_position.0 - grab_offset);
                }
            }
            WindowEvent::MouseInput {
                state: mouse_state,
                button: winit::event::MouseButton::Left,
                ..
            } => match mouse_state {
                winit::event::ElementState::Pressed => {
                    let (x, y) = state.cursor_position;
                    if state.handle_horizontal_scrollbar_press(x, y) {
                        state.window.request_redraw();
                    }
                }
                winit::event::ElementState::Released => {
                    state.horizontal_scrollbar_drag = None;
                }
            },
            WindowEvent::ScaleFactorChanged {
                inner_size_writer: _,
                ..
            } => {
                let new_size = state.window.inner_size();
                state.resize(new_size.width, new_size.height);
                state.window.request_redraw();
            }
            _ => {}
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let database_url = "";
    println!("Connecting to PostgreSQL...");
    let pool = PgPoolOptions::new()
        .max_connections(20)
        .connect(database_url)
        .await
        .context("Couldn't connect to database")?;
    println!("Connected to PostgreSQL.");

    let event_loop = EventLoop::new().unwrap();
    let mut app = App::new(pool);

    event_loop.run_app(&mut app)?;
    Ok(())
}
