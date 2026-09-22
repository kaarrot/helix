use helix_core::Position;
use helix_view::{
    graphics::{CursorKind, Rect},
    Editor,
};
use tui::buffer::Buffer;

use crate::compositor::{Component, Context, Event, EventResult};
use crate::ui::picker::MIN_AREA_WIDTH_FOR_SIDE_BY_SIDE;

/// Contains a component placed in the center of the parent component
pub struct Overlay<T> {
    /// Child component
    pub content: T,
    /// Function to compute the size and position of the child component
    pub calc_child_size: Box<dyn Fn(Rect) -> Rect>,
}

/// Surrounds the component with a margin of 5% on each side, and an additional 2 rows at the bottom.
///
/// Narrow or tall terminals use the full remaining area so a stacked picker
/// preview still has room after the 50/50 split.
pub fn overlaid<T>(content: T) -> Overlay<T> {
    Overlay {
        content,
        calc_child_size: Box::new(overlay_area),
    }
}

pub(crate) fn overlay_area(rect: Rect) -> Rect {
    let rect = rect.clip_bottom(2);
    if rect.width < MIN_AREA_WIDTH_FOR_SIDE_BY_SIDE || rect.height > rect.width {
        rect
    } else {
        clip_rect_relative(rect, 90, 90)
    }
}

fn clip_rect_relative(rect: Rect, percent_horizontal: u8, percent_vertical: u8) -> Rect {
    fn mul_and_cast(size: u16, factor: u8) -> u16 {
        ((size as u32) * (factor as u32) / 100).try_into().unwrap()
    }

    let inner_w = mul_and_cast(rect.width, percent_horizontal);
    let inner_h = mul_and_cast(rect.height, percent_vertical);

    let offset_x = rect.width.saturating_sub(inner_w) / 2;
    let offset_y = rect.height.saturating_sub(inner_h) / 2;

    Rect {
        x: rect.x + offset_x,
        y: rect.y + offset_y,
        width: inner_w,
        height: inner_h,
    }
}

impl<T: Component + 'static> Component for Overlay<T> {
    fn render(&mut self, area: Rect, frame: &mut Buffer, ctx: &mut Context) {
        let dimensions = (self.calc_child_size)(area);
        self.content.render(dimensions, frame, ctx)
    }

    fn required_size(&mut self, (width, height): (u16, u16)) -> Option<(u16, u16)> {
        let area = Rect {
            x: 0,
            y: 0,
            width,
            height,
        };
        let dimensions = (self.calc_child_size)(area);
        let viewport = (dimensions.width, dimensions.height);
        let _ = self.content.required_size(viewport)?;
        Some((width, height))
    }

    fn handle_event(&mut self, event: &Event, ctx: &mut Context) -> EventResult {
        self.content.handle_event(event, ctx)
    }

    fn cursor(&self, area: Rect, ctx: &Editor) -> (Option<Position>, CursorKind) {
        let dimensions = (self.calc_child_size)(area);
        self.content.cursor(dimensions, ctx)
    }

    fn id(&self) -> Option<&'static str> {
        self.content.id()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn narrow_terminal_uses_full_overlay() {
        let area = overlay_area(Rect::new(0, 0, 40, 24));
        assert_eq!(
            area,
            Rect::new(0, 0, 40, 22),
            "phone-sized terminals should not lose 10% to overlay margins"
        );
    }

    #[test]
    fn short_narrow_terminal_still_keeps_statusline_rows() {
        let area = overlay_area(Rect::new(0, 0, 40, 18));
        assert_eq!(area, Rect::new(0, 0, 40, 16));
        assert!(
            area.height >= crate::ui::picker::MIN_AREA_HEIGHT_FOR_VERTICAL_PREVIEW,
            "a 18-row Termux screen must still be tall enough for a stacked preview"
        );
    }

    #[test]
    fn wide_short_terminal_keeps_centered_margin() {
        let area = overlay_area(Rect::new(0, 0, 120, 40));
        assert_eq!(area.width, 108);
        assert_eq!(area.height, 34);
        assert!(area.x > 0 && area.y > 0);
    }

    #[test]
    fn tall_terminal_uses_full_overlay() {
        let area = overlay_area(Rect::new(0, 0, 80, 100));
        assert_eq!(area, Rect::new(0, 0, 80, 98));
    }
}
