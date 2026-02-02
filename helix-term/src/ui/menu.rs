use crate::{
    compositor::{Callback, Component, Compositor, Context, Event, EventResult},
    ctrl, key, shift,
};
use tui::{buffer::Buffer as Surface, widgets::Table};

pub use tui::widgets::{Cell, Row};

use helix_view::{editor::SmartTabConfig, graphics::Rect, Editor};
use tui::layout::Constraint;

pub trait Item: Sync + Send + 'static {
    /// Additional editor state that is used for label calculation.
    type Data: Sync + Send + 'static;

    fn format(&self, data: &Self::Data) -> Row<'_>;
}

pub type MenuCallback<T> = Box<dyn Fn(&mut Editor, Option<&T>, MenuEvent)>;

pub struct Menu<T: Item> {
    options: Vec<T>,
    editor_data: T::Data,

    cursor: Option<usize>,

    /// (index, score)
    matches: Vec<(u32, u32)>,

    widths: Vec<Constraint>,

    callback_fn: MenuCallback<T>,

    scroll: usize,
    size: (u16, u16),
    viewport: (u16, u16),
    recalculate: bool,
    /// Whether to automatically select the first item
    auto_select_first: bool,
}

impl<T: Item> Menu<T> {
    const LEFT_PADDING: usize = 1;

    // TODO: it's like a slimmed down picker, share code? (picker = menu + prompt with different
    // rendering)
    pub fn new(
        options: Vec<T>,
        editor_data: <T as Item>::Data,
        callback_fn: impl Fn(&mut Editor, Option<&T>, MenuEvent) + 'static,
    ) -> Self {
        let matches: Vec<(u32, u32)> = (0..options.len() as u32).map(|i| (i, 0)).collect();
        Self {
            options,
            editor_data,
            matches,
            cursor: None,
            widths: Vec::new(),
            callback_fn: Box::new(callback_fn),
            scroll: 0,
            size: (0, 0),
            viewport: (0, 0),
            recalculate: true,
            auto_select_first: false,
        }
    }

    /// Enable or disable auto-selection of the first item
    pub fn set_auto_select_first(&mut self, auto_select: bool) {
        self.auto_select_first = auto_select;
        if auto_select && self.cursor.is_none() && !self.matches.is_empty() {
            self.cursor = Some(0);
        }
    }

    pub fn reset_cursor(&mut self) {
        self.cursor = if self.auto_select_first && !self.matches.is_empty() {
            Some(0)
        } else {
            None
        };
        self.scroll = 0;
        self.recalculate = true;
    }

    pub fn update_options(&mut self) -> (&mut Vec<(u32, u32)>, &mut Vec<T>) {
        self.recalculate = true;
        (&mut self.matches, &mut self.options)
    }

    pub fn ensure_cursor_in_bounds(&mut self) {
        if self.matches.is_empty() {
            self.cursor = None;
            self.scroll = 0;
        } else {
            self.scroll = 0;
            self.recalculate = true;
            if let Some(cursor) = &mut self.cursor {
                *cursor = (*cursor).min(self.matches.len() - 1)
            } else if self.auto_select_first {
                // Auto-select first item if nothing is selected and auto_select is enabled
                self.cursor = Some(0);
            }
        }
    }

    pub fn clear(&mut self) {
        self.matches.clear();

        // reset cursor position
        self.cursor = None;
        self.scroll = 0;
    }

    pub fn move_up(&mut self) {
        let len = self.matches.len();
        let max_index = len.saturating_sub(1);
        let pos = self.cursor.map_or(max_index, |i| (i + max_index) % len) % len;
        self.cursor = Some(pos);
        self.adjust_scroll();
    }

    pub fn move_down(&mut self) {
        let len = self.matches.len();
        let pos = self.cursor.map_or(0, |i| i + 1) % len;
        self.cursor = Some(pos);
        self.adjust_scroll();
    }

    fn recalculate_size(&mut self, viewport: (u16, u16)) {
        let n = self
            .options
            .first()
            .map(|option| option.format(&self.editor_data).cells.len())
            .unwrap_or_default();
        let max_lens = self.options.iter().fold(vec![0; n], |mut acc, option| {
            let row = option.format(&self.editor_data);
            // maintain max for each column
            for (acc, cell) in acc.iter_mut().zip(row.cells.iter()) {
                let width = cell.content.width();
                if width > *acc {
                    *acc = width;
                }
            }

            acc
        });

        let height = self.matches.len().min(10).min(viewport.1 as usize);
        // do all the matches fit on a single screen?
        let fits = self.matches.len() <= height;

        let mut len = max_lens.iter().sum::<usize>() + n;

        if !fits {
            len += 1; // +1: reserve some space for scrollbar
        }

        len += Self::LEFT_PADDING;
        let width = len.min(viewport.0 as usize);

        self.widths = max_lens
            .into_iter()
            .map(|len| Constraint::Length(len as u16))
            .collect();

        self.size = (width as u16, height as u16);

        // adjust scroll offsets if size changed
        self.adjust_scroll();
        self.recalculate = false;
    }

    fn adjust_scroll(&mut self) {
        let win_height = self.size.1 as usize;
        if let Some(cursor) = self.cursor {
            let mut scroll = self.scroll;
            if cursor > (win_height + scroll).saturating_sub(1) {
                // scroll down
                scroll += cursor - (win_height + scroll).saturating_sub(1)
            } else if cursor < scroll {
                // scroll up
                scroll = cursor
            }
            self.scroll = scroll;
        }
    }

    pub fn selection(&self) -> Option<&T> {
        self.cursor.and_then(|cursor| {
            self.matches
                .get(cursor)
                .map(|(index, _score)| &self.options[*index as usize])
        })
    }

    pub fn selection_mut(&mut self) -> Option<&mut T> {
        self.cursor.and_then(|cursor| {
            self.matches
                .get(cursor)
                .map(|(index, _score)| &mut self.options[*index as usize])
        })
    }

    pub fn is_empty(&self) -> bool {
        self.matches.is_empty()
    }

    pub fn len(&self) -> usize {
        self.matches.len()
    }
}

impl<T: Item + PartialEq> Menu<T> {
    pub fn replace_option(&mut self, old_option: &impl PartialEq<T>, new_option: T) {
        for option in &mut self.options {
            if old_option == option {
                *option = new_option;
                break;
            }
        }
    }
}

use super::PromptEvent as MenuEvent;

impl<T: Item + 'static> Component for Menu<T> {
    fn handle_event(&mut self, event: &Event, cx: &mut Context) -> EventResult {
        let event = match event {
            Event::Key(event) => *event,
            _ => return EventResult::Ignored(None),
        };

        let close_fn: Option<Callback> = Some(Box::new(|compositor: &mut Compositor, _| {
            // remove the layer
            compositor.pop();
        }));

        // Ignore tab key when supertab is turned on in order not to interfere
        // with it. (Is there a better way to do this?)
        if (event == key!(Tab) || event == shift!(Tab))
            && cx.editor.config().auto_completion
            && matches!(
                cx.editor.config().smart_tab,
                Some(SmartTabConfig {
                    enable: true,
                    supersede_menu: true,
                })
            )
        {
            return EventResult::Ignored(None);
        }

        match event {
            // esc or ctrl-c aborts the completion and closes the menu
            key!(Esc) | ctrl!('c') => {
                (self.callback_fn)(cx.editor, self.selection(), MenuEvent::Abort);
                return EventResult::Consumed(close_fn);
            }
            // arrow up/ctrl-p/shift-tab prev completion choice (including updating the doc)
            shift!(Tab) | key!(Up) | ctrl!('p') => {
                self.move_up();
                (self.callback_fn)(cx.editor, self.selection(), MenuEvent::Update);
                return EventResult::Consumed(None);
            }
            key!(Tab) | key!(Down) | ctrl!('n') => {
                // arrow down/ctrl-n/tab advances completion choice (including updating the doc)
                self.move_down();
                (self.callback_fn)(cx.editor, self.selection(), MenuEvent::Update);
                return EventResult::Consumed(None);
            }
            key!(Enter) => {
                if let Some(selection) = self.selection() {
                    (self.callback_fn)(cx.editor, Some(selection), MenuEvent::Validate);
                    return EventResult::Consumed(close_fn);
                } else {
                    return EventResult::Ignored(close_fn);
                }
            }
            // KeyEvent {
            //     code: KeyCode::Char(c),
            //     modifiers: KeyModifiers::NONE,
            // } => {
            //     self.insert_char(c);
            //     (self.callback_fn)(cx.editor, &self.line, MenuEvent::Update);
            // }

            // / -> edit_filter?
            //
            // enter confirms the match and closes the menu
            // typing filters the menu
            // if we run out of options the menu closes itself
            _ => (),
        }
        // for some events, we want to process them but send ignore, specifically all input except
        // tab/enter/ctrl-k or whatever will confirm the selection/ ctrl-n/ctrl-p for scroll.
        // EventResult::Consumed(None)
        EventResult::Ignored(None)
    }

    fn required_size(&mut self, viewport: (u16, u16)) -> Option<(u16, u16)> {
        if viewport != self.viewport || self.recalculate {
            self.recalculate_size(viewport);
        }

        Some(self.size)
    }

    fn render(&mut self, area: Rect, surface: &mut Surface, cx: &mut Context) {
        let theme = &cx.editor.theme;
        let style = theme
            .try_get("ui.menu")
            .unwrap_or_else(|| theme.get("ui.text"));
        let selected = theme.get("ui.menu.selected");

        surface.clear_with(area, style);

        let scroll = self.scroll;

        let options: Vec<_> = self
            .matches
            .iter()
            .map(|(index, _score)| {
                // (index, self.options.get(*index).unwrap()) // get_unchecked
                &self.options[*index as usize] // get_unchecked
            })
            .collect();

        let len = options.len();

        let win_height = area.height as usize;

        let rows = options
            .iter()
            .map(|option| option.format(&self.editor_data));
        let table = Table::new(rows)
            .style(style)
            .highlight_style(selected)
            .column_spacing(1)
            .widths(&self.widths);

        use tui::widgets::TableState;

        table.render_table(
            area.clip_left(Self::LEFT_PADDING as u16).clip_right(1),
            surface,
            &mut TableState {
                offset: scroll,
                selected: self.cursor,
            },
            false,
        );

        let render_borders = cx.editor.menu_border();

        if !render_borders {
            if let Some(cursor) = self.cursor {
                let offset_from_top = cursor - scroll;
                let left = &mut surface[(area.left(), area.y + offset_from_top as u16)];
                left.set_style(selected);
                let right = &mut surface[(
                    area.right().saturating_sub(1),
                    area.y + offset_from_top as u16,
                )];
                right.set_style(selected);
            }
        }

        let fits = len <= win_height;

        let scroll_style = theme.get("ui.menu.scroll");
        if !fits {
            let scroll_height = win_height.pow(2).div_ceil(len).min(win_height);
            let scroll_line = (win_height - scroll_height) * scroll
                / std::cmp::max(1, len.saturating_sub(win_height));

            let mut cell;
            for i in 0..win_height {
                cell = &mut surface[(area.right() - 1, area.top() + i as u16)];

                let half_block = if render_borders { "▌" } else { "▐" };

                if scroll_line <= i && i < scroll_line + scroll_height {
                    // Draw scroll thumb
                    cell.set_symbol(half_block);
                    cell.set_fg(scroll_style.fg.unwrap_or(helix_view::theme::Color::Reset));
                } else if !render_borders {
                    // Draw scroll track
                    cell.set_symbol(half_block);
                    cell.set_fg(scroll_style.bg.unwrap_or(helix_view::theme::Color::Reset));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Minimal Item implementation for testing
    #[derive(Clone, Debug, PartialEq)]
    struct TestItem(String);

    impl Item for TestItem {
        type Data = ();

        fn format(&self, _data: &Self::Data) -> Row<'_> {
            Row::new(vec![Cell::from(self.0.as_str())])
        }
    }

    fn dummy_callback(_editor: &mut Editor, _item: Option<&TestItem>, _event: MenuEvent) {}

    #[test]
    fn test_menu_auto_select_disabled_by_default() {
        let items = vec![
            TestItem("item1".to_string()),
            TestItem("item2".to_string()),
        ];
        let menu = Menu::new(items, (), dummy_callback);

        // Auto-select should be disabled by default
        assert!(!menu.auto_select_first);
        // Cursor should be None
        assert_eq!(menu.cursor, None);
    }

    #[test]
    fn test_menu_set_auto_select() {
        let items = vec![
            TestItem("item1".to_string()),
            TestItem("item2".to_string()),
        ];
        let mut menu = Menu::new(items, (), dummy_callback);

        // Enable auto-select
        menu.set_auto_select_first(true);

        assert!(menu.auto_select_first);
        // Should auto-select first item
        assert_eq!(menu.cursor, Some(0));
    }

    #[test]
    fn test_reset_cursor_with_auto_select() {
        let items = vec![
            TestItem("item1".to_string()),
            TestItem("item2".to_string()),
        ];
        let mut menu = Menu::new(items, (), dummy_callback);

        menu.set_auto_select_first(true);
        menu.cursor = Some(1); // Move to second item

        menu.reset_cursor();

        // Should reset to first item when auto_select is true
        assert_eq!(menu.cursor, Some(0));
    }

    #[test]
    fn test_reset_cursor_without_auto_select() {
        let items = vec![
            TestItem("item1".to_string()),
            TestItem("item2".to_string()),
        ];
        let mut menu = Menu::new(items, (), dummy_callback);

        menu.cursor = Some(1);
        menu.reset_cursor();

        // Should reset to None when auto_select is false
        assert_eq!(menu.cursor, None);
    }

    #[test]
    fn test_ensure_cursor_in_bounds_with_auto_select() {
        let items = vec![
            TestItem("item1".to_string()),
            TestItem("item2".to_string()),
        ];
        let mut menu = Menu::new(items, (), dummy_callback);

        menu.set_auto_select_first(true);
        menu.cursor = None;

        menu.ensure_cursor_in_bounds();

        // Should auto-select first item
        assert_eq!(menu.cursor, Some(0));
    }

    #[test]
    fn test_ensure_cursor_in_bounds_without_auto_select() {
        let items = vec![
            TestItem("item1".to_string()),
            TestItem("item2".to_string()),
        ];
        let mut menu = Menu::new(items, (), dummy_callback);

        menu.cursor = None;
        menu.ensure_cursor_in_bounds();

        // Should remain None
        assert_eq!(menu.cursor, None);
    }
}
