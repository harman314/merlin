//! Full-window viewer for photos and videos.
//!
//! Attachments open here instead of the desktop's file handler. Zoom and pan
//! live in egui memory, so the view keeps them without touching app state.

use egui::{Color32, Rect, Sense, Vec2, pos2, vec2};

use crate::animation;
use crate::app::App;
use crate::model::{Action, Content};
use crate::theme::{self, Icon};

/// Height of the bar holding the title and controls.
const BAR: f32 = 56.0;
/// Room for the traffic lights when macOS draws them over the content.
const MACOS_INSET: f32 = 80.0;
const MIN_ZOOM: f32 = 0.2;
const MAX_ZOOM: f32 = 8.0;

/// Zoom and pan for one attachment.
#[derive(Clone, Copy)]
struct Transform {
    zoom: f32,
    offset: Vec2,
}

impl Default for Transform {
    fn default() -> Self {
        Self {
            zoom: 1.0,
            offset: Vec2::ZERO,
        }
    }
}

pub fn show(app: &mut App, ctx: &egui::Context) {
    let Some((chat, id)) = app.viewer.clone() else {
        return;
    };
    // The file is gone when the message was deleted or the archive reloaded.
    let Some(open) = describe(app, &chat, &id) else {
        app.actions.push(Action::CloseViewer);
        return;
    };
    let palette = app.palette;
    let screen = ctx.content_rect();
    let transform_id = egui::Id::new(("viewer-transform", &id));
    let mut transform: Transform = ctx
        .data(|data| data.get_temp(transform_id))
        .unwrap_or_default();
    let mut actions: Vec<Action> = Vec::new();

    egui::Area::new(egui::Id::new("viewer"))
        .order(egui::Order::Foreground)
        .fixed_pos(screen.min)
        .show(ctx, |ui| {
            ui.set_min_size(screen.size());
            ui.painter()
                .rect_filled(screen, 0.0, Color32::from_black_alpha(235));
            let canvas = Rect::from_min_max(pos2(screen.left(), screen.top() + BAR), screen.max);

            // A click that misses the picture closes the viewer, as it does on
            // the phone. The picture takes the drag so panning still works.
            let backdrop = ui.interact(
                canvas,
                ui.id().with("viewer-backdrop"),
                Sense::click_and_drag(),
            );
            let drawn = paint(ui, &open, canvas, transform, palette.secondary);

            if let Some(rect) = drawn {
                if backdrop.dragged() {
                    transform.offset += backdrop.drag_delta();
                }
                if backdrop.double_clicked() {
                    transform = Transform::default();
                } else if backdrop.clicked()
                    && !rect.contains(backdrop.interact_pointer_pos().unwrap_or(rect.center()))
                {
                    actions.push(Action::CloseViewer);
                }
                let scroll = ui.input(|input| input.smooth_scroll_delta.y);
                if backdrop.hovered() && scroll != 0.0 {
                    transform.zoom =
                        (transform.zoom * (1.0 + scroll * 0.002)).clamp(MIN_ZOOM, MAX_ZOOM);
                }
            } else if backdrop.clicked() {
                actions.push(Action::CloseViewer);
            }

            bar(ui, app, &open, screen, &mut actions, &mut transform);
            arrows(ui, &open, canvas, &mut actions, palette.text);
        });

    ctx.data_mut(|data| data.insert_temp(transform_id, transform));
    app.actions.append(&mut actions);
}

/// The attachment the viewer is showing.
struct Open {
    path: std::path::PathBuf,
    title: String,
    caption: Option<String>,
    video: bool,
    document: bool,
    /// Position among the chat's viewable attachments, one-based.
    at: usize,
    total: usize,
}

/// Collects what the view needs, so the borrow ends before actions are pushed.
fn describe(app: &App, chat: &str, id: &str) -> Option<Open> {
    let message = app.conversations.get(chat)?.message(id)?;
    let (media, caption, video, document, named) = match &message.content {
        Content::Image { media, caption } => (media, caption.clone(), false, false, None),
        Content::Video { media, caption, .. } => (media, caption.clone(), true, false, None),
        Content::Document {
            media,
            caption,
            file_name,
            ..
        } => (media, caption.clone(), false, true, Some(file_name.clone())),
        _ => return None,
    };
    let path = media.path.clone()?;
    let ids = app.viewable(chat);
    let at = ids.iter().position(|known| known == id)?;
    let title = named.unwrap_or_else(|| {
        path.file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Attachment".to_owned())
    });
    Some(Open {
        path,
        title,
        caption,
        video,
        document,
        at: at + 1,
        total: ids.len(),
    })
}

/// Draws the picture or video frame and returns the rectangle it filled.
fn paint(
    ui: &mut egui::Ui,
    open: &Open,
    canvas: Rect,
    transform: Transform,
    dim: Color32,
) -> Option<Rect> {
    // Documents render through the system thumbnailer rather than a bundled engine.
    if open.document {
        return match crate::quicklook::preview(ui.ctx(), &open.path, canvas.height()) {
            crate::quicklook::Preview::Ready(texture) => {
                let size = texture.size_vec2();
                let rect = fit(canvas, size, transform);
                ui.painter().image(
                    texture.id(),
                    rect,
                    Rect::from_min_max(egui::Pos2::ZERO, pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
                Some(rect)
            }
            crate::quicklook::Preview::Pending => {
                theme::paint_spinner(ui, canvas, 28.0, dim);
                None
            }
            crate::quicklook::Preview::Unavailable => {
                ui.painter().text(
                    canvas.center(),
                    egui::Align2::CENTER_CENTER,
                    "No preview for this document. Use Open in default app.",
                    theme::regular(13.0),
                    dim,
                );
                None
            }
        };
    }
    // Video and animated pictures decode in process; the still loader handles the rest.
    if open.video {
        let fitted = fit(canvas, vec2(16.0, 9.0), transform);
        return match animation::frame(ui, &open.path, fitted) {
            animation::Frame::Ready(texture) => {
                let size = texture.size();
                let rect = fit(canvas, vec2(size[0] as f32, size[1] as f32), transform);
                ui.painter().image(
                    texture.id(),
                    rect,
                    Rect::from_min_max(egui::Pos2::ZERO, pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
                Some(rect)
            }
            animation::Frame::Pending => {
                theme::paint_spinner(ui, canvas, 28.0, dim);
                None
            }
            animation::Frame::Unavailable => {
                ui.painter().text(
                    canvas.center(),
                    egui::Align2::CENTER_CENTER,
                    "This video cannot be played here. Use Open in default app.",
                    theme::regular(13.0),
                    dim,
                );
                None
            }
        };
    }
    let image = egui::Image::new(crate::util::image_uri(&open.path));
    match image.load_for_size(ui.ctx(), canvas.size()) {
        Ok(egui::load::TexturePoll::Ready { texture }) => {
            let rect = fit(canvas, texture.size, transform);
            image.paint_at(ui, rect);
            Some(rect)
        }
        Ok(egui::load::TexturePoll::Pending { .. }) => {
            theme::paint_spinner(ui, canvas, 28.0, dim);
            None
        }
        Err(_) => {
            ui.painter().text(
                canvas.center(),
                egui::Align2::CENTER_CENTER,
                "This picture cannot be shown here. Use Open in default app.",
                theme::regular(13.0),
                dim,
            );
            None
        }
    }
}

/// Centres the natural size inside the canvas, scaled to fit and then zoomed.
fn fit(canvas: Rect, natural: Vec2, transform: Transform) -> Rect {
    let natural = vec2(natural.x.max(1.0), natural.y.max(1.0));
    let scale = (canvas.width() / natural.x).min(canvas.height() / natural.y);
    let size = natural * scale * transform.zoom;
    Rect::from_center_size(canvas.center() + transform.offset, size)
}

/// Title, counter, and the controls along the top.
fn bar(
    ui: &mut egui::Ui,
    app: &App,
    open: &Open,
    screen: Rect,
    actions: &mut Vec<Action>,
    transform: &mut Transform,
) {
    let palette = app.palette;
    let rect = Rect::from_min_max(screen.min, pos2(screen.right(), screen.top() + BAR));
    ui.painter()
        .rect_filled(rect, 0.0, Color32::from_black_alpha(140));
    let left = if theme::macos_chrome(ui.ctx()) {
        rect.left() + MACOS_INSET
    } else {
        rect.left() + 16.0
    };
    ui.painter().text(
        pos2(left, rect.center().y),
        egui::Align2::LEFT_CENTER,
        &open.title,
        theme::medium(13.0),
        palette.text,
    );
    if open.total > 1 {
        ui.painter().text(
            pos2(left, rect.center().y + 15.0),
            egui::Align2::LEFT_CENTER,
            format!("{} of {}", open.at, open.total),
            theme::regular(11.0),
            palette.secondary,
        );
    }
    if let Some(caption) = &open.caption {
        ui.painter().text(
            pos2(screen.center().x, screen.bottom() - 28.0),
            egui::Align2::CENTER_CENTER,
            caption,
            theme::regular(13.0),
            palette.text,
        );
    }
    let mut ui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(Rect::from_min_max(
                pos2(rect.right() - 210.0, rect.top()),
                rect.max,
            ))
            .layout(egui::Layout::right_to_left(egui::Align::Center)),
    );
    let ui = &mut ui;
    if theme::icon_button(ui, Icon::X, 18.0, palette.text, palette.accent, "Close").clicked() {
        actions.push(Action::CloseViewer);
    }
    if theme::icon_button(
        ui,
        Icon::ExternalLink,
        17.0,
        palette.text,
        palette.accent,
        "Open in default app",
    )
    .clicked()
    {
        actions.push(Action::OpenFile(open.path.clone()));
    }
    if theme::icon_button(
        ui,
        Icon::Plus,
        17.0,
        palette.text,
        palette.accent,
        "Zoom in",
    )
    .clicked()
    {
        transform.zoom = (transform.zoom * 1.25).clamp(MIN_ZOOM, MAX_ZOOM);
    }
    if theme::icon_button(
        ui,
        Icon::Minus,
        17.0,
        palette.text,
        palette.accent,
        "Zoom out",
    )
    .clicked()
    {
        transform.zoom = (transform.zoom / 1.25).clamp(MIN_ZOOM, MAX_ZOOM);
    }
    if theme::icon_button(
        ui,
        Icon::Maximize,
        17.0,
        palette.text,
        palette.accent,
        "Fit to window",
    )
    .clicked()
    {
        *transform = Transform::default();
    }
}

/// Previous and next controls down the sides.
fn arrows(ui: &mut egui::Ui, open: &Open, canvas: Rect, actions: &mut Vec<Action>, color: Color32) {
    if open.total <= 1 {
        return;
    }
    let edge = Vec2::splat(44.0);
    if open.at > 1 {
        let rect = Rect::from_center_size(pos2(canvas.left() + 34.0, canvas.center().y), edge);
        if step(ui, rect, Icon::ChevronLeft, color, "Previous") {
            actions.push(Action::StepViewer(-1));
        }
    }
    if open.at < open.total {
        let rect = Rect::from_center_size(pos2(canvas.right() - 34.0, canvas.center().y), edge);
        if step(ui, rect, Icon::ChevronRight, color, "Next") {
            actions.push(Action::StepViewer(1));
        }
    }
}

fn step(ui: &mut egui::Ui, rect: Rect, icon: Icon, color: Color32, tooltip: &str) -> bool {
    let response = ui
        .interact(rect, ui.id().with(tooltip), Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    let alpha = if response.hovered() { 190 } else { 120 };
    ui.painter()
        .circle_filled(rect.center(), 22.0, Color32::from_black_alpha(alpha));
    theme::paint_icon(ui, icon, rect, 20.0, color);
    response.on_hover_text(tooltip).clicked()
}
